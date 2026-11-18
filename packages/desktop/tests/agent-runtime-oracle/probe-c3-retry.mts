// C3 Task 0：流恢复 / 重试的黑盒观察（5xx、流中断、401、重试期间取消）。
//
// 用法：pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c3-retry.mts
//
// 注入点只在本地 mock provider（`mock-provider.mts` 的 C3 扩展）：
//   - `failure.kind:"http"` 回 401/5xx；
//   - `failure.kind:"abort", phase:"mid_stream"` 在写出若干 delta 后断开；
//   - 第二个参数 `context.attempt` 让同一请求第一次失败、重试成功。
// 运行时的实现体不读不改；所有 expect 来自本脚本记录的帧/应答/DB。
import { readdir, readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

import type { JsonObject } from "./session-lifecycle-fixtures.mjs";
import {
  eventSummary,
  isolationRecord,
  normalizeC3Deep,
  projectEvents,
  projectGoalDb,
  startC3Harness,
  textDeltas,
  writeC3Fixture,
  type C3Harness,
} from "./probe-c3-support.mjs";
import { delay } from "./probe-c2-support.mjs";

const FIXTURE_ROOT = fileURLToPath(new URL("./fixtures/", import.meta.url));

type RequestProjection = {
  path: string;
  stream: boolean | undefined;
  toolCount: number;
  assistantCount: number;
};

function projectRequests(harness: C3Harness): RequestProjection[] {
  return harness.requests.map(({ path, body }) => ({
    path,
    stream: typeof body.stream === "boolean" ? body.stream : undefined,
    toolCount: Array.isArray(body.tools) ? body.tools.length : 0,
    assistantCount: Array.isArray(body.messages)
      ? (body.messages as JsonObject[]).filter((message) => message.role === "assistant").length
      : 0,
  }));
}

async function conversation(harness: C3Harness): Promise<JsonObject[]> {
  const result = (await harness.session.request("session/messages", {
    sessionId: harness.session.sessionId,
  })) as { messages?: JsonObject[] };
  const normalized = normalizeC3Deep(
    result.messages ?? [],
    harness.context,
    harness.aliases,
  ) as JsonObject[];
  return normalized.slice(-5).map((message) => {
    const info = (message.info ?? {}) as JsonObject;
    const parts = Array.isArray(message.parts) ? (message.parts as JsonObject[]) : [];
    return {
      role: info.role ?? null,
      finish: info.finish ?? null,
      semanticsKind: (info.semantics as JsonObject | undefined)?.kind ?? null,
      text: parts
        .filter((part) => part.type === "text")
        .map((part) => String(part.text ?? ""))
        .join(""),
    };
  });
}

async function retryCase(
  name: string,
  options: {
    note: string;
    script: Parameters<typeof startC3Harness>[0]["script"];
    input: JsonObject;
    afterTerminal?: (harness: C3Harness, start: number) => Promise<JsonObject>;
  },
): Promise<JsonObject> {
  const harness = await startC3Harness({
    prefix: `c3-retry-${name}`,
    script: options.script,
  });
  try {
    const start = harness.eventCount();
    await harness.session.send("retry scenario");
    await harness.waitForEvent(
      start,
      (event) => event.type === "turn.completed" || event.type === "turn.failed",
      `${name} 终态`,
      90_000,
    );
    await delay(300);
    const events = harness.eventsSince(start);
    const extra = options.afterTerminal ? await options.afterTerminal(harness, start) : {};
    return {
      isolation: isolationRecord(harness),
      requests: projectRequests(harness),
      events: projectEvents(events),
      eventSummary: eventSummary(events),
      textDeltas: textDeltas(events),
      debug: await harness.readDebug(),
      conversation: await conversation(harness),
      db: projectGoalDb(harness.readTables()),
      ...extra,
    };
  } finally {
    await harness.stop();
  }
}

/** Case 1：503 → 重试成功；记录 apiRetry 投影、事件顺序与模型用量行。 */
async function caseHttpRetry(): Promise<JsonObject> {
  return retryCase("http-503", {
    note: "第一次模型请求 503，adapter 退避后第二次成功；turn.completed 恰好一次。",
    script: [
      (_body, context) =>
        context.attempt === 0
          ? { failure: { kind: "http", status: 503, message: "upstream overloaded" } }
          : { text: "answer after one retry" },
    ],
    input: {
      scenario: "http-503-then-success",
      attempts: 2,
      failure: { status: 503, message: "upstream overloaded" },
    },
  });
}

/** Case 2：3 次 503 后成功；观察退避序列（数值在夹具里折叠为 <retry-delay>）。 */
async function caseBackoffSequence(): Promise<JsonObject> {
  const fixture = await retryCase("http-503-backoff", {
    note: "前三次模型请求 503，第四次成功；重试间隔是带抖动的指数退避（数值运行相关）。",
    script: [
      (_body, context) =>
        context.attempt < 3
          ? { failure: { kind: "http", status: 503, message: "upstream overloaded" } }
          : { text: "answer after three retries" },
    ],
    input: {
      scenario: "http-503-backoff-then-success",
      attempts: 4,
      failures: 3,
      failure: { status: 503, message: "upstream overloaded" },
    },
  });
  const debug = fixture.debug as JsonObject;
  const entries = Array.isArray(debug.networkEntries) ? (debug.networkEntries as JsonObject[]) : [];
  return {
    ...fixture,
    retrySchedule: {
      attempts: entries.filter((entry) => entry.statusType === "model_request_started").length,
      retryScheduled: entries.filter((entry) => entry.statusType === "model_retry_scheduled")
        .length,
      normalizedDelays: entries
        .filter((entry) => entry.statusType === "model_retry_scheduled")
        .map((entry) => entry.delayMs ?? null),
    },
  };
}

/** Case 3：流中断（已提交一个 text delta 后断连）→ streamRecovery → 重试成功。 */
async function caseStreamAbort(): Promise<JsonObject> {
  return retryCase("stream-abort", {
    note:
      "第一次模型流在写出 text delta 后断开（streamOutputCommitted=true）：运行时先发 " +
      "streamRecovery.updated 锚点/丢弃计数，再发同 attempt 的新请求；已提交 delta 不重复。",
    script: [
      (_body, context) =>
        context.attempt === 0
          ? {
              textChunks: ["partial text ", "that is interrupted"],
              chunkDelayMs: 500,
              failure: { kind: "abort", phase: "mid_stream", afterDeltas: 1 },
            }
          : { text: "complete answer after recovery" },
    ],
    input: {
      scenario: "mid-stream-abort-then-success",
      abort: {
        phase: "mid_stream",
        afterDeltas: 1,
        chunks: ["partial text ", "that is interrupted"],
      },
      recoveryText: "complete answer after recovery",
    },
  });
}

/** Case 4：401 非 retryable；一次 turn.failed，不重试。 */
async function caseAuthFailure(): Promise<JsonObject> {
  return retryCase("auth-401", {
    note: "模型端点回 401（auth_failed，retryable=false）：无重试、无 retry_scheduled，turn.failed 恰好一次。",
    script: [
      (_body, context) =>
        context.attempt === 0
          ? { failure: { kind: "http", status: 401, message: "invalid api key" } }
          : { text: "must not run" },
    ],
    input: {
      scenario: "http-401-non-retryable",
      failure: { status: 401, message: "invalid api key" },
    },
  });
}

/** Case 5：重试退避期间 session/stop；不再发起新尝试，终态 cancelled。 */
async function caseCancelDuringRetry(): Promise<JsonObject> {
  const harness = await startC3Harness({
    prefix: "c3-retry-cancel",
    script: [
      (_body, context) =>
        context.attempt === 0
          ? { failure: { kind: "http", status: 503, message: "upstream overloaded" } }
          : { text: "must not run after cancel" },
    ],
  });
  try {
    const start = harness.eventCount();
    await harness.session.send("cancel during retry");
    await harness.waitForEvent(
      start,
      (event) => {
        const payload = (event.payload ?? {}) as JsonObject;
        return event.type === "session.updated" && payload.type === "model_retry_scheduled";
      },
      "model_retry_scheduled",
      60_000,
    );
    const requestsBeforeStop = harness.requests.length;
    let stop: JsonObject;
    try {
      stop = (await harness.session.request("session/stop", {
        sessionId: harness.session.sessionId,
      })) as JsonObject;
    } catch (error) {
      stop = { error: String(error) };
    }
    await harness.waitForEvent(
      start,
      (event) => {
        const payload = (event.payload ?? {}) as JsonObject;
        return event.type === "turn.completed" && payload.resultType === "cancelled";
      },
      "cancel 后 cancelled 终态",
      60_000,
    );
    await delay(2000);
    const events = harness.eventsSince(start);
    return {
      isolation: isolationRecord(harness),
      stop,
      requestsBeforeStop,
      requestsAfterStopWindow: harness.requests.length,
      requests: projectRequests(harness),
      events: projectEvents(events),
      eventSummary: eventSummary(events),
      debug: await harness.readDebug(),
      db: projectGoalDb(harness.readTables()),
    };
  } finally {
    await harness.stop();
  }
}

/** 从既有 C1/C2 夹具提取 `streamRecovery.updated` 的去重形状。 */
async function extractC1C2Shapes(): Promise<JsonObject> {
  const shapes = new Map<string, { payload: JsonObject; occurrences: JsonObject[] }>();
  const sources: string[] = [];
  for (const directory of [FIXTURE_ROOT, `${FIXTURE_ROOT}c2/`]) {
    const entries = await readdir(directory, { withFileTypes: true });
    for (const entry of entries) {
      if (!entry.isFile() || !entry.name.endsWith(".json")) continue;
      const relative = directory === FIXTURE_ROOT ? entry.name : `c2/${entry.name}`;
      const fixture = JSON.parse(await readFile(`${directory}${entry.name}`, "utf8")) as JsonObject;
      const expect = (fixture.expect ?? {}) as JsonObject;
      const events = Array.isArray(expect.events) ? (expect.events as JsonObject[]) : [];
      let count = 0;
      for (const event of events) {
        if (event.type !== "streamRecovery.updated") continue;
        count += 1;
        const payload = event.payload as JsonObject;
        const key = JSON.stringify(payload);
        const existing = shapes.get(key) ?? { payload, occurrences: [] };
        existing.occurrences.push({ fixture: relative, count: existing.occurrences.length + 1 });
        shapes.set(key, existing);
      }
      if (count > 0) sources.push(relative);
    }
  }
  // 每个 fixture 的出现次数按同形状聚合，避免把同一行重复写成多个 occurrence。
  for (const shape of shapes.values()) {
    const counts = new Map<string, number>();
    for (const occurrence of shape.occurrences) {
      const fixture = String(occurrence.fixture);
      counts.set(fixture, (counts.get(fixture) ?? 0) + 1);
    }
    shape.occurrences = [...counts.entries()].map(([fixture, count]) => ({ fixture, count }));
  }
  const payloads = [...shapes.values()].sort((left, right) =>
    JSON.stringify(left.payload).localeCompare(JSON.stringify(right.payload)),
  );
  return {
    case: "stream-recovery-c1-c2-shapes",
    note:
      "从既有 C1/C2 夹具机器提取的 streamRecovery.updated 去重形状；只读既有夹具，不修改它们。" +
      "C3 注入流中断产生的四种新形状记录在 retry-stream-abort.json 的 events 里。",
    input: { fixtureRoot: "packages/desktop/tests/agent-runtime-oracle/fixtures", sources },
    expect: {
      shapeCount: payloads.length,
      shapes: payloads,
    },
  };
}

const fixtures: Record<string, JsonObject> = {
  "retry-5xx-then-success": {
    case: "retry-5xx-then-success",
    note: "C3 Task 0 oracle 捕获：503 → retry_scheduled（apiRetry attempt=1/maxRetries=10）→ 第二次成功。",
    input: {
      scenario: "http-503-then-success",
      scriptKind: "attempt-aware step function",
      failure: { status: 503, message: "upstream overloaded" },
      verifier: null,
    },
    expect: await caseHttpRetry(),
  },
  "retry-backoff-sequence": {
    case: "retry-backoff-sequence",
    note: "C3 Task 0 oracle 捕获：连续 3 次 503 的退避序列；间隔带抖动，夹具折叠为 <retry-delay>。",
    input: {
      scenario: "http-503-backoff-then-success",
      scriptKind: "attempt-aware step function",
      failures: 3,
      failure: { status: 503, message: "upstream overloaded" },
    },
    expect: await caseBackoffSequence(),
  },
  "retry-stream-abort": {
    case: "retry-stream-abort",
    note:
      "C3 Task 0 oracle 捕获：流中断（已提交 delta）触发 streamRecovery.updated 四种形状，" +
      "随后同一 logical request 重新请求并成功；已提交 delta 不重复。",
    input: {
      scenario: "mid-stream-abort-then-success",
      scriptKind: "attempt-aware step function",
      abort: {
        phase: "mid_stream",
        afterDeltas: 1,
        chunks: ["partial text ", "that is interrupted"],
      },
    },
    expect: await caseStreamAbort(),
  },
  "retry-401-non-retryable": {
    case: "retry-401-non-retryable",
    note: "C3 Task 0 oracle 捕获：401 auth_failed 非 retryable；无重试，turn.failed 一次。",
    input: {
      scenario: "http-401-non-retryable",
      scriptKind: "attempt-aware step function",
      failure: { status: 401, message: "invalid api key" },
    },
    expect: await caseAuthFailure(),
  },
  "retry-cancel-during-backoff": {
    case: "retry-cancel-during-backoff",
    note:
      "C3 Task 0 oracle 捕获：503 的 retry_scheduled 之后 session/stop；取消优先于重试，" +
      "只有 1 次 provider 请求，turn.completed(cancelled) 一次。",
    input: {
      scenario: "cancel-during-backoff",
      scriptKind: "attempt-aware step function",
      failure: { status: 503, message: "upstream overloaded" },
      cancel: "session/stop after model_retry_scheduled",
    },
    expect: await caseCancelDuringRetry(),
  },
  "stream-recovery-c1-c2-shapes": await extractC1C2Shapes(),
};

const targets: string[] = [];
for (const [name, fixture] of Object.entries(fixtures)) {
  targets.push(await writeC3Fixture(name, fixture));
}
process.stdout.write(
  `${JSON.stringify(
    {
      fixtures: targets,
      summaries: Object.fromEntries(
        Object.entries(fixtures).map(([name, fixture]) => [
          name,
          {
            types: (fixture.expect as JsonObject)?.eventSummary
              ? ((fixture.expect as JsonObject).eventSummary as JsonObject).types
              : null,
            requests: (fixture.expect as JsonObject)?.requests ?? null,
            retrySchedule: (fixture.expect as JsonObject)?.retrySchedule ?? null,
          },
        ]),
      ),
    },
    null,
    2,
  )}\n`,
);
