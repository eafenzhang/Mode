// C3 Task 5：retry / background / subagents 夹具回放的 case drivers（比较面见 replay-c3.mts 文件头）。
//
// 剧本重建：夹具 `input` 里的剧本描述（attempt-aware / user-text-keyed step function）在
// probe 里是函数、JSON 只存参数——这里按同一语义重建（与 probe-c3-retry/background 的
// 步骤逐字对应；应答文本取 probe 源面，比较仍指向夹具录制值）。

import type { JsonObject } from "./session-lifecycle-fixtures.mjs";
import type { MockStep } from "./mock-provider.mjs";
import {
  normalizeC3Deep,
  normalizeError,
  projectGoalDb,
} from "./probe-c3-support.mjs";
import { delay } from "./probe-c2-support.mjs";
import {
  diffC3Value,
  diffEventStream,
  diffGoalDb,
  foldMessageSummary,
  RETRY_EVENT_SCOPE,
  startC3Replay,
  stopC3Replay,
  type C3Diff,
  type C3Fixture,
  type C3ReplayResult,
  type C3Run,
} from "./replay-c3.mjs";

// ---------------------------------------------------------------------------
// 剧本重建（attempt-aware：第 N 次失败、重试成功；user-text-keyed：按 user 文本分支）
// ---------------------------------------------------------------------------

function httpRetryScript(
  failure: { status: number; message: string },
  successText: string,
  failures: number,
): MockStep[] {
  return [
    (_body, context) =>
      context.attempt < failures
        ? { failure: { kind: "http", status: failure.status, message: failure.message } }
        : { text: successText },
  ];
}

function streamAbortScript(chunks: string[], recoveryText: string, afterDeltas: number): MockStep[] {
  return [
    (_body, context) =>
      context.attempt === 0
        ? {
            textChunks: chunks,
            chunkDelayMs: 500,
            failure: { kind: "abort", phase: "mid_stream", afterDeltas },
          }
        : { text: recoveryText },
  ];
}

type BackgroundSpec = { command: string; description: string };

export function backgroundScript(
  spawnPrompt: string,
  backgrounds: BackgroundSpec[],
  busyPrompt: string | undefined,
  busyDelayMs: number,
  defaultAnswer: string,
): MockStep[] {
  return [
    (body) => {
      const text = lastUserText(body);
      if (text.includes(spawnPrompt)) {
        return {
          toolCalls: backgrounds.map((background) => ({
            name: "Bash",
            input: {
              command: background.command,
              run_in_background: true,
              description: background.description,
            },
          })),
        };
      }
      if (busyPrompt !== undefined && text.includes(busyPrompt)) {
        return { text: "busy answer", delayMs: busyDelayMs };
      }
      if (text.includes("<task-notification>")) {
        return { text: busyPrompt !== undefined ? "notification handled" : "notification turn acknowledged" };
      }
      return { text: defaultAnswer };
    },
  ];
}

function lastUserText(body: JsonObject): string {
  const messages = Array.isArray(body.messages) ? (body.messages as JsonObject[]) : [];
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (message.role !== "user") continue;
    if (typeof message.content === "string") return message.content;
    if (Array.isArray(message.content)) {
      return (message.content as JsonObject[])
        .map((part) => (typeof part.text === "string" ? part.text : ""))
        .join("\n");
    }
  }
  return "";
}

// ---------------------------------------------------------------------------
// retry 共用驱动面（5xx / 401 / stream-abort / backoff / cancel）
// ---------------------------------------------------------------------------

type RetryExpect = {
  requests?: JsonObject[];
  events?: JsonObject[];
  textDeltas?: string[];
  conversation?: JsonObject[];
  db?: JsonObject;
  stop?: JsonObject;
  requestsBeforeStop?: number;
  requestsAfterStopWindow?: number;
};

function projectRequests(run: C3Run): JsonObject[] {
  return run.rawBodies.map((body) => ({
    path: "/v1/messages",
    stream: typeof body.stream === "boolean" ? body.stream : undefined,
    // 工具清单条数不比：会话工具清单属 C1 工具面对齐面（登记），两侧同折。
    toolCount: "<tool-list>",
    assistantCount: Array.isArray(body.messages)
      ? (body.messages as JsonObject[]).filter((message) => message.role === "assistant").length
      : 0,
  }));
}

/** mock 请求投影的登记折叠（两侧同一实现）。 */
function foldRequests(requests: JsonObject[]): JsonObject[] {
  return requests.map((request) => ({ ...request, toolCount: "<tool-list>" }));
}

function requestDiffs(diffs: C3Diff[], path: string, actual: JsonObject[], expected: JsonObject[]): void {
  diffs.push(...diffC3Value("rows", path, actual, foldRequests(expected)));
}

async function conversationTail(run: C3Run): Promise<JsonObject[]> {
  const result = (await run.session.request("session/messages", {
    sessionId: run.session.sessionId,
  })) as { messages?: JsonObject[] };
  const normalized = normalizeC3Deep(
    result.messages ?? [],
    run.context,
    run.aliases,
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

function textDeltasOf(events: { type: string; payload: unknown }[]): string[] {
  return events
    .filter((event) => event.type === "model.streaming")
    .map((event) => (event.payload ?? {}) as JsonObject)
    .filter((payload) => payload.kind === "text_delta" && typeof payload.delta === "string")
    .map((payload) => payload.delta as string);
}

/** conversation 的登记折叠：timeline_event 合成行两侧同删（C1 时代 #22/#23）。 */
export function foldConversation(rows: JsonObject[]): JsonObject[] {
  return rows.filter((row) => row.semanticsKind !== "timeline_event");
}

async function replayRetry(
  fixture: C3Fixture,
  options: {
    script: MockStep[];
    prompt?: string;
    cancelDuringBackoff?: boolean;
    keepTextDeltas?: boolean;
  },
): Promise<C3ReplayResult> {
  const diffs: C3Diff[] = [];
  const notes: string[] = [];
  const run = await startC3Replay({ fixture, mockScript: options.script });
  try {
    const expect = fixture.expect as RetryExpect;
    const start = run.frames().length;
    await run.session.send(options.prompt ?? "retry scenario");
    if (options.cancelDuringBackoff === true) {
      await run.waitForEvent(
        start,
        (event) => {
          const payload = (event.payload ?? {}) as JsonObject;
          return event.type === "session.updated" && payload.type === "model_retry_scheduled";
        },
        "model_retry_scheduled",
        90_000,
      );
      const requestsBeforeStop = run.rawBodies.length;
      let stop: JsonObject;
      try {
        stop = (await run.session.request("session/stop", {
          sessionId: run.session.sessionId,
        })) as JsonObject;
      } catch (error) {
        stop = { error: String(error) };
      }
      await run.waitForEvent(
        start,
        (event) => {
          const payload = (event.payload ?? {}) as JsonObject;
          return event.type === "turn.completed" && payload.resultType === "cancelled";
        },
        "cancel 后 cancelled 终态",
        90_000,
      );
      await delay(2000);
      diffs.push(...diffC3Value("response", "stop", stop, expect.stop ?? null));
      diffs.push(...diffC3Value("rows", "requestsBeforeStop", requestsBeforeStop, expect.requestsBeforeStop ?? null));
      diffs.push(...diffC3Value("rows", "requestsAfterStopWindow", run.rawBodies.length, expect.requestsAfterStopWindow ?? null));
    } else {
      await run.waitForTerminals(run.terminalCount() + 1);
    }
    await delay(300);
    const events = run.frames().slice(start);
    diffs.push(...diffEventStream(run.normalizedActualEvents(start, RETRY_EVENT_SCOPE), run.normalizedFixtureEvents((expect.events as JsonObject[]) ?? [], RETRY_EVENT_SCOPE)));
    requestDiffs(diffs, "requests", projectRequests(run), (expect.requests ?? []) as JsonObject[]);
    if (options.keepTextDeltas === true && expect.textDeltas !== undefined) {
      diffs.push(...diffC3Value("rows", "textDeltas", textDeltasOf(events), expect.textDeltas));
    }
    if (expect.conversation !== undefined) {
      diffs.push(...diffC3Value("rows", "conversation", foldConversation(await conversationTail(run)), foldConversation(expect.conversation as JsonObject[])));
    }
    diffs.push(...diffGoalDb("db", projectGoalDb(run.readTables()), (expect.db ?? {}) as JsonObject, { dropModelUsage: true }));
    notes.push("model_usage 整表不比：C1 普通 turn 不写（§2.7 登记）；session/debug.networkEntries 不比（未产出）");
    if (run.session.invalidLines.length > 0) {
      throw new Error(`replay-c3：stdout 解析失败行：${run.session.invalidLines[0]}`);
    }
    return { case: fixture.case, diffs, notes, stderr: run.session.stderr };
  } finally {
    await stopC3Replay(run);
  }
}

// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// subagents-empty（只读面边界；spawn 场景归 C4，见 §C3 登记）
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// subagents-empty（只读面边界；spawn 场景归 C4，见 §C3 登记）
// ---------------------------------------------------------------------------

function redactCursor(result: JsonObject): JsonObject {
  const ended = result.ended as JsonObject | undefined;
  if (ended !== undefined && typeof ended.nextCursor === "string") {
    ended.nextCursor = "<cursor>";
  }
  return result;
}

async function callSubagents(
  run: C3Run,
  params: Record<string, unknown> = {},
): Promise<{ ok: true; result: JsonObject } | { ok: false; error: JsonObject }> {
  try {
    const raw = (await run.session.request("session/subagents", {
      sessionId: run.session.sessionId,
      ...params,
    })) as JsonObject;
    return { ok: true, result: redactCursor(normalizeC3Deep(raw, run.context, run.aliases) as JsonObject) };
  } catch (error) {
    return { ok: false, error: normalizeError(error, run.context, run.aliases) };
  }
}

/** subagents DB 投影的登记折叠（两侧同一实现）：session title（#26）、messageSummary 合成行。 */
function foldSubagentDb(db: JsonObject): JsonObject {
  const sessions = ((db.sessions as JsonObject[] | undefined) ?? []).map((row) => ({
    ...row,
    // C1 时代 title 差异（§C2.8 #26）：oracle title=首条输入，Rust title 空。
    title: "<title>",
  }));
  return {
    sessions,
    sessionTaskLink: db.sessionTaskLink ?? null,
    messages: foldMessageSummary(db.messages ?? []) as JsonObject[],
  };
}

function projectSubagentDb(run: C3Run): JsonObject {
  const tables = run.readTables();
  const sessions = ((tables.session as JsonObject | undefined)?.rows as JsonObject[] | undefined) ?? [];
  const links = ((tables.session_task_link as JsonObject | undefined)?.rows as JsonObject[] | undefined) ?? [];
  return foldSubagentDb({
    sessions: sessions.map((row) => ({
      id: row.id,
      parent_id: row.parent_id,
      task_type: row.task_type,
      title: row.title,
      slug: row.slug,
      directory: row.directory,
    })),
    sessionTaskLink: links,
    messages: projectGoalDb(tables).messageSummary as JsonObject[],
  }) as JsonObject;
}

async function replaySubagentsEmpty(fixture: C3Fixture): Promise<C3ReplayResult> {
  const diffs: C3Diff[] = [];
  const notes: string[] = [];
  const run = await startC3Replay({ fixture, mockScript: [{ text: "plain answer" }] });
  try {
    const expect = fixture.expect;
    const beforeAnyTurn = await callSubagents(run, { endedLimit: 20 });
    let badSessionError: JsonObject | null = null;
    try {
      await run.session.request("session/subagents", { sessionId: "sess_missing", endedLimit: 20 });
    } catch (error) {
      badSessionError = normalizeError(error, run.context, run.aliases);
    }
    await run.session.send("plain prompt");
    await run.waitForTerminals(run.terminalCount() + 1);
    await delay(200);
    const emptyWithLimit = await callSubagents(run, { endedLimit: 20 });
    const emptyDefaultLimit = await callSubagents(run);
    const zeroLimit = await callSubagents(run, { endedLimit: 0 });
    const negativeLimit = await callSubagents(run, { endedLimit: -1 });
    diffs.push(...diffC3Value("response", "beforeAnyTurn", beforeAnyTurn, expect.beforeAnyTurn as JsonObject));
    diffs.push(...diffC3Value("response", "badSessionError", badSessionError, expect.badSessionError as JsonObject));
    for (const [key, value] of [
      ["emptyWithLimit", emptyWithLimit],
      ["emptyDefaultLimit", emptyDefaultLimit],
      ["zeroLimit", zeroLimit],
      ["negativeLimit", negativeLimit],
    ] as const) {
      diffs.push(...diffC3Value("response", key, value, (expect[key] as JsonObject) ?? null));
    }
    diffs.push(...diffC3Value("db", "db", projectSubagentDb(run), foldSubagentDb((expect.db ?? {}) as JsonObject)));
    if (run.session.invalidLines.length > 0) {
      throw new Error(`replay-c3：stdout 解析失败行：${run.session.invalidLines[0]}`);
    }
    return { case: fixture.case, diffs, notes, stderr: run.session.stderr };
  } finally {
    await stopC3Replay(run);
  }
}

// ---------------------------------------------------------------------------
// 注册表
// ---------------------------------------------------------------------------

export function registerC3MixedCases(registry: Record<string, (fixture: C3Fixture) => Promise<C3ReplayResult>>): void {
  registry["retry-5xx-then-success"] = (fixture) =>
    replayRetry(fixture, {
      script: httpRetryScript(fixture.input.failure as { status: number; message: string }, "answer after one retry", 1),
      keepTextDeltas: true,
    });
  registry["retry-401-non-retryable"] = (fixture) =>
    replayRetry(fixture, {
      script: httpRetryScript(fixture.input.failure as { status: number; message: string }, "must not run", 1),
      keepTextDeltas: true,
    });
  registry["retry-stream-abort"] = (fixture) => {
    const abort = fixture.input.abort as { chunks: string[] };
    return replayRetry(fixture, {
      script: streamAbortScript(abort.chunks, "complete answer after recovery", 1),
      keepTextDeltas: true,
    });
  };
  registry["retry-cancel-during-backoff"] = (fixture) =>
    replayRetry(fixture, {
      script: httpRetryScript(fixture.input.failure as { status: number; message: string }, "must not run after cancel", 1),
      prompt: "cancel during retry",
      cancelDuringBackoff: true,
    });
  registry["subagents-empty"] = replaySubagentsEmpty;
}
