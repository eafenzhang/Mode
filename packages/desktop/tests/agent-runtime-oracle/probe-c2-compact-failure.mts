// C2 Task 0：`session/compact` 的模型失败形态（provider 返回不可用的 compact 应答）。
//
// 用法：pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c2-compact-failure.mts
//
// 两种失败都由 mock provider 的**剧本**产生（不是手写事件）：
//   1. compact 请求返回一个 tool_use（compact 只允许文本）；
//   2. compact 请求返回空 toolCalls（没有文本内容）。
// 记录 accepted 应答之后的异步 turn.failed、事件、DB（保留原历史 + failed timeline 行）。
import { normalizeDeep } from "./session-lifecycle-fixtures.mjs";
import {
  readC2Db,
  startC2Harness,
  stripErrorStack,
  waitForTerminals,
  writeC2Fixture,
} from "./probe-c2-support.mjs";
import type { JsonObject } from "./session-lifecycle-fixtures.mjs";

const SEED = { "hello.txt": "hello\n" };
const PROMPTS = ["first prompt", "second prompt"];
const COMPACT_MARKER = "Respond with TEXT ONLY";

function failureScript(mode: "tool-call" | "empty"): any[] {
  return [
    (body: JsonObject) => {
      const serialized = JSON.stringify(body);
      if (serialized.includes(COMPACT_MARKER)) {
        if (mode === "empty") return { toolCalls: [] };
        return { toolCalls: [{ name: "Read", input: { file_path: "not-a-real-file.txt" } }] };
      }
      return { text: "normal turn answer" };
    },
  ];
}

function summaryCounts(db: JsonObject): JsonObject {
  const messages = ((db.message as JsonObject).rows as JsonObject[]) ?? [];
  const parts = ((db.part as JsonObject).rows as JsonObject[]) ?? [];
  return {
    messages: ((db.message as JsonObject).rowCount as number) ?? null,
    compactSummaryMessages: messages.filter((row) => {
      const data = row.data as JsonObject;
      return (data.semantics as JsonObject | undefined)?.kind === "compact_summary";
    }).length,
    compactionParts: parts.filter((row) => (row.data as JsonObject)?.type === "compaction").length,
    failedTimelineMessages: messages.filter((row) => (row.data as JsonObject)?.finish === "failed")
      .length,
  };
}

async function runFailure(mode: "tool-call" | "empty", prefix: string): Promise<JsonObject> {
  const harness = await startC2Harness({
    prefix,
    script: failureScript(mode),
    seed: SEED,
  });
  try {
    for (const prompt of PROMPTS) await harness.sendAndWait(prompt);
    const before = readC2Db(harness.dbPath, harness.context);
    const boundary = harness.captureState.events.length;
    const compact = (await harness.session.request("session/compact", {
      sessionId: harness.session.sessionId,
      inputId: `input_compact_${mode}`,
    })) as JsonObject;
    await waitForTerminals(harness.captureState, 3);
    const events = harness.captureState.events
      .slice(boundary)
      .map((event) => ({ ...event, payload: stripErrorStack(event.payload) }));
    const after = readC2Db(harness.dbPath, harness.context);
    const isolation = harness.isolation();
    const normalizedCompact = normalizeDeep(compact, harness.context) as JsonObject;
    return {
      isolation,
      mode,
      compactResponse: {
        response: normalizedCompact.response,
        compact: normalizedCompact.compact ?? null,
        snapshotRuntime: (normalizedCompact.snapshot as JsonObject | undefined)?.runtime,
      },
      events,
      eventTypes: events.map((event) => event.type),
      failureEvent: events.find((event) => event.type === "turn.failed") ?? null,
      dbBefore: summaryCounts(before),
      dbAfter: summaryCounts(after),
      sessionAfter: ((after.session as JsonObject).rows as JsonObject[])[0],
      turnUsageLast: ((after.turn_usage as JsonObject).rows as JsonObject[]).at(-1),
      modelUsageLast: ((after.model_usage as JsonObject).rows as JsonObject[]).at(-1),
      compactRequestBody: harness.bodies.at(-1) ?? null,
    };
  } finally {
    await harness.stop();
  }
}

const toolCall = await runFailure("tool-call", "compact-failure-tool");
const empty = await runFailure("empty", "compact-failure-empty");
const targets = [
  await writeC2Fixture("compact-provider-tool-call", {
    case: "compact-provider-tool-call",
    note:
      "C2 Task 0 oracle 捕获：compact 模型请求被 mock provider 回成 tool_use；session/compact " +
      "先 accepted，随后异步 turn.failed（MODEL_ERROR，Tool use is not allowed during compaction）。" +
      "DB 保留原历史并写一条 failed timeline 行，没有 compact_summary。",
    input: { seed: SEED, prompts: PROMPTS, compactMarker: COMPACT_MARKER },
    expect: toolCall,
  }),
  await writeC2Fixture("compact-provider-empty", {
    case: "compact-provider-empty",
    note:
      "C2 Task 0 oracle 捕获：compact 模型请求被 mock provider 回成空 toolCalls（无文本）；" +
      "session/compact 先 accepted，随后异步 turn.failed（MODEL_ERROR，retryable=true）。",
    input: { seed: SEED, prompts: PROMPTS, compactMarker: COMPACT_MARKER },
    expect: empty,
  }),
];
process.stdout.write(
  `${JSON.stringify(
    {
      fixtures: targets,
      toolCall: {
        compactResponse: toolCall.compactResponse,
        eventTypes: toolCall.eventTypes,
        failure: (toolCall.failureEvent as JsonObject | null)?.payload,
        dbBefore: toolCall.dbBefore,
        dbAfter: toolCall.dbAfter,
        turnUsageLast: toolCall.turnUsageLast,
        modelUsageLast: toolCall.modelUsageLast,
      },
      empty: {
        eventTypes: empty.eventTypes,
        failure: (empty.failureEvent as JsonObject | null)?.payload,
        dbAfter: empty.dbAfter,
      },
    },
    null,
    2,
  )}\n`,
);
