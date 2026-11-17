// C2 Task 0：黑盒捕获 `session/compact`（手动 compact、instructions、expectedRevision）。
//
// 用法：
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c2-compact.mts
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c2-compact.mts --keep
//
// 纪律（docs/specs/clean-room-policy.md）：只运行 oracle、观察线上帧与数据库行，不读实现体。
// 隔离（tauri-agent-runtime.md §1.4）：driver 注入 MODE_DATA_BASE_DIR + HOME/USERPROFILE/MODE_HOME，
// 并断言 `<隔离根>/.mode/.mode-root.json` 与 `<隔离根>/.mode/cli/log` 都存在；不写真实 ~/.mode。
// 期望值只来自本次运行：夹具由本脚本投影/规范化后写出，不含手写 expect。
import {
  delay,
  readC2Db,
  readSessionColumn,
  startC2Harness,
  stripErrorStack,
  writeC2Fixture,
} from "./probe-c2-support.mjs";
import { normalizeDeep, type JsonObject } from "./session-lifecycle-fixtures.mjs";

const SEED = { "hello.txt": "hello\n" };
const PROMPTS = [
  "first prompt: introduce the workspace",
  "second prompt: summarize the project",
  "third prompt: list the open questions",
  "fourth prompt: propose next steps",
];
const SCRIPT = [
  { text: "first turn answer: workspace introduced" },
  { text: "second turn answer: project summarized" },
  { text: "third turn answer: questions listed" },
  { text: "fourth turn answer: next steps proposed" },
  { text: "post compact answer: continuing after summary" },
];
const INSTRUCTIONS = "Focus on decisions and open questions; keep identifiers verbatim.";
const COMPACT_INPUT = "/compact Focus on decisions and open questions; keep identifiers verbatim.";

/** 应答快照只保留判定字段，避免与 DB/事件夹具三份重复。 */
function projectSnapshot(snapshot: JsonObject): JsonObject {
  const messages = Array.isArray(snapshot.messages) ? (snapshot.messages as JsonObject[]) : [];
  return {
    keys: Object.keys(snapshot).sort(),
    session: snapshot.session,
    runtime: snapshot.runtime,
    projection: snapshot.projection,
    messageRoles: messages.map((message) => (message.info as JsonObject)?.role),
    messageFinishes: messages.map((message) => (message.info as JsonObject)?.finish ?? null),
    messagePartTypes: messages.map((message) =>
      Array.isArray(message.parts) ? (message.parts as JsonObject[]).map((part) => part.type) : [],
    ),
  };
}

function projectCompactResponse(response: JsonObject): JsonObject {
  return {
    response: response.response,
    compact: response.compact ?? null,
    compactKeys: response.compact === undefined ? [] : Object.keys(response.compact as JsonObject),
    snapshot: projectSnapshot(response.snapshot as JsonObject),
  };
}

function compactRequests(requests: readonly JsonObject[]): JsonObject[] {
  return requests.filter((request) => request.method === "session/compact");
}

async function manualCompact(): Promise<JsonObject> {
  const harness = await startC2Harness({ prefix: "compact-manual", script: SCRIPT, seed: SEED });
  try {
    for (const prompt of PROMPTS) await harness.sendAndWait(prompt);
    const readBefore = (await harness.session.request("session/read", {
      sessionId: harness.session.sessionId,
    })) as JsonObject;
    const revision = (readBefore.runtime as JsonObject | undefined)?.stateRevision as
      | number
      | undefined;
    const boundary = harness.captureState.events.length;
    let sampleCount = 0;
    const sampleValues = new Set<string>();
    let settled = false;
    const compactPromise = harness.session.request("session/compact", {
      sessionId: harness.session.sessionId,
      inputId: "input_compact_manual_1",
      instructions: INSTRUCTIONS,
      ...(revision === undefined ? {} : { expectedRevision: revision }),
    });
    const poll = (async () => {
      while (!settled) {
        sampleCount += 1;
        sampleValues.add(String(readSessionColumn(harness.dbPath, "time_compacting") ?? "null"));
        await delay(2);
      }
    })();
    let compact: JsonObject;
    try {
      compact = (await compactPromise) as JsonObject;
    } finally {
      settled = true;
      await poll;
    }
    await delay(500);
    const events = harness.captureState.events
      .slice(boundary)
      .map((event) => ({ ...event, payload: stripErrorStack(event.payload) }));
    const dbAfterCompact = readC2Db(harness.dbPath, harness.context);
    const bodiesAfterCompact = harness.bodies.length;
    await harness.sendAndWait("post compact prompt");
    const isolation = harness.isolation();
    const normalizedCompact = normalizeDeep(compact, harness.context) as JsonObject;
    const normalizedReadBefore = normalizeDeep(readBefore, harness.context) as JsonObject;
    return {
      isolation,
      readBefore: {
        runtime: normalizedReadBefore.runtime,
        projection: normalizedReadBefore.projection,
        messageCount: Array.isArray(normalizedReadBefore.messages)
          ? normalizedReadBefore.messages.length
          : null,
      },
      expectedRevision: revision ?? null,
      compactResponse: projectCompactResponse(normalizedCompact),
      events,
      eventTypes: events.map((event) => event.type),
      timeCompactingSamples: {
        sampled: sampleCount > 0,
        distinctValues: [...sampleValues].sort(),
      },
      timeCompactingNonEmptySamples: [...sampleValues].filter((value) => value !== "null").length,
      requests: compactRequests(harness.captureState.requests),
      dbAfterCompact,
      dbAfterNextTurn: readC2Db(harness.dbPath, harness.context),
      compactModelRequest: harness.bodies[bodiesAfterCompact - 1] ?? null,
      postCompactModelRequests: harness.bodies.slice(bodiesAfterCompact),
      notes: { compactInput: COMPACT_INPUT, instructions: INSTRUCTIONS },
    };
  } finally {
    await harness.stop();
  }
}

async function expectedRevisionConflict(): Promise<JsonObject> {
  const harness = await startC2Harness({ prefix: "compact-revision", script: SCRIPT, seed: SEED });
  try {
    await harness.sendAndWait("revision prompt");
    const read = (await harness.session.request("session/read", {
      sessionId: harness.session.sessionId,
    })) as JsonObject;
    const revision = ((read.runtime as JsonObject | undefined)?.stateRevision as number) ?? 0;
    const before = readC2Db(harness.dbPath, harness.context);
    const boundary = harness.captureState.events.length;
    let rejected: string | undefined;
    try {
      await harness.session.request("session/compact", {
        sessionId: harness.session.sessionId,
        expectedRevision: revision + 1000,
      });
    } catch (error) {
      rejected = harness.context.dataRoot
        ? String(error instanceof Error ? error.message : String(error)).replaceAll(
            harness.context.dataRoot,
            "<data-root>",
          )
        : String(error);
    }
    await delay(300);
    const isolation = harness.isolation();
    return {
      isolation,
      actualRevision: revision,
      requestedRevision: revision + 1000,
      rejected,
      requests: compactRequests(harness.captureState.requests),
      events: harness.captureState.events.slice(boundary),
      eventTypes: harness.captureState.events.slice(boundary).map((event) => event.type),
      messageRowsBefore: (before.message as JsonObject).rowCount,
      messageRowsAfter: (readC2Db(harness.dbPath, harness.context).message as JsonObject).rowCount,
    };
  } finally {
    await harness.stop();
  }
}

async function compactWithoutInstructions(): Promise<JsonObject> {
  const harness = await startC2Harness({ prefix: "compact-plain", script: SCRIPT, seed: SEED });
  try {
    await harness.sendAndWait(PROMPTS[0] as string);
    await harness.sendAndWait(PROMPTS[1] as string);
    const boundary = harness.captureState.events.length;
    const compact = (await harness.session.request("session/compact", {
      sessionId: harness.session.sessionId,
      inputId: "input_compact_plain_1",
    })) as JsonObject;
    await delay(500);
    const isolation = harness.isolation();
    const events = harness.captureState.events
      .slice(boundary)
      .map((event) => ({ ...event, payload: stripErrorStack(event.payload) }));
    return {
      isolation,
      compactResponse: projectCompactResponse(
        normalizeDeep(compact, harness.context) as JsonObject,
      ),
      events,
      eventTypes: events.map((event) => event.type),
      requests: compactRequests(harness.captureState.requests),
      dbAfterCompact: readC2Db(harness.dbPath, harness.context),
      compactRequestBody: harness.bodies.at(-1) ?? null,
    };
  } finally {
    await harness.stop();
  }
}

const manual = await manualCompact();
const revision = await expectedRevisionConflict();
const plain = await compactWithoutInstructions();
const targets = [
  await writeC2Fixture("compact-manual", {
    case: "compact-manual",
    note:
      "C2 Task 0 oracle 捕获：4 个文本 turn 后调用 session/compact（带 instructions 与有效 " +
      "expectedRevision），随后再发一个 turn；记录应答/事件/DB/model_request 与 time_compacting 采样。",
    input: {
      seed: SEED,
      prompts: [...PROMPTS, "post compact prompt"],
      script: SCRIPT,
      instructions: INSTRUCTIONS,
    },
    expect: manual,
  }),
  await writeC2Fixture("compact-no-instructions", {
    case: "compact-no-instructions",
    note:
      "C2 Task 0 oracle 捕获：2 个文本 turn 后调用 session/compact（不带 instructions / " +
      "expectedRevision），记录默认摘要提示词与 customInstructions=false 的 DB 形状。",
    input: { seed: SEED, prompts: PROMPTS.slice(0, 2), script: SCRIPT },
    expect: plain,
  }),
  await writeC2Fixture("compact-expected-revision", {
    case: "compact-expected-revision",
    note:
      "C2 Task 0 oracle 捕获：session/compact 带过期 expectedRevision，记录错误应答与" +
      "「没有新增 compact 行/事件」的事实。",
    input: { seed: SEED, prompts: ["revision prompt"], script: SCRIPT },
    expect: revision,
  }),
];
process.stdout.write(
  `${JSON.stringify(
    {
      fixtures: targets,
      compactResponse: manual.compactResponse,
      timeCompactingSamples: manual.timeCompactingSamples,
      timeCompactingNonEmptySamples: manual.timeCompactingNonEmptySamples,
      eventTypes: manual.eventTypes,
      revision,
      plain: {
        compactResponse: plain.compactResponse,
        events: plain.eventTypes,
        customInstructions: ((plain.dbAfterCompact as JsonObject).part as JsonObject).rows
          ? ((plain.dbAfterCompact as JsonObject).part as JsonObject).rows
              .map(
                (row) =>
                  ((row.data as JsonObject)?.compactBoundary as JsonObject | undefined)
                    ?.customInstructions ?? null,
              )
              .filter((value) => value !== null)
          : [],
      },
    },
    null,
    2,
  )}\n`,
);
