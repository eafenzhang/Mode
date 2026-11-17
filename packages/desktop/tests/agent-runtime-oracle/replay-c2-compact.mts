// C2 Task 8：compact 夹具回放的 case drivers（`replay-c2.mts` 的比较半边；
// 共享投影/折叠在 `replay-c2-compare.mts`，规则登记见 `replay-c2.mts` 文件头）。
//
// 驱动用与 oracle 录制同形状的**本地 mock provider**（`mock-provider.mts`，剧本
// 语义同为「请求下标 = assistant 条数」）：模型能力事实（contextWindow 等）来自
// provider 配置，夹具的快照/投影面才可比；scripted 装配（C1 三层回放用）没有能力
// 事实，`projection.contextWindow`/`settings.model.available` 会结构性不同。
//
// 逐 case 的比较面（`agent-c2-contract.md` 登记的可比子集；登记差异两侧同删）：
// - `compact-manual` / `compact-no-instructions`：受理应答形状（§1.1/§1.9）+ 事件
//   （登记缺省同删后只剩 turn.started/turn.completed）+ compact 增量 DB 行（§1.3）
//   + mock 请求路径数（isolation）+ `time_compacting` 恒 null / `time_updated` 前进。
// - `compact-expected-revision`：`-32009` 错误形状 + 零事件 + 前后全行逐表相等
//   + 列定义（schema；T7 对齐面）。
// - `compact-same-tick`：单次 trial（时序相关，§1.6）：第一单恒 accepted；第二单
//   两种形状都是登记结果——`already_running`（计数 = trial 1）或 `accepted→skipped`
//   （计数 = trial 4 + skipped 行逐字段）。观察注记写 `notes`，不算差异。
// - `compact-provider-tool-call` / `compact-provider-empty`：accepted 应答 + 异步
//   `turn.failed` 载荷逐键（§1.8）+ compact 增量行 + 失败计数关系。
//
// **DB 比较面（登记）**：只取 C2 拥有的行——compact 增量的 message/part 行、
// turn_usage/model_usage 的最后一行、整表不变的 tool_usage/session_input、session 行
// （title 三列折叠）。prompt turn 的行形状（会话 timeline 行、user/assistant 行字段、
// per-turn model_usage、session_entry 第三行）是 **C1 时代的 DB 层差异**——此前任何
// 门禁都没比过 DB，T8 新发现并登记（`tauri-agent-runtime.md` §C2.8「DB 层未对齐面」）；
// post turn 之后只比行数增量（message +2 / part +4 / turn_usage +1）。
import type { JsonObject } from "./session-lifecycle-fixtures.mjs";
import type { MockStep } from "./mock-provider.mjs";
import {
  acceptedResponseDiffs,
  diffCompactAddedRows,
  diffTableShapes,
  foldRowPosition,
  foldSessionRow,
  normalizeDeep,
  parseRpcError,
  rowsOf,
  summaryCounts,
  type DbSnapshot,
} from "./replay-c2-compare.mjs";
import {
  diffDbRows,
  diffDeep,
  diffEventStream,
  readReplayDb,
  readStateRevision,
  sendTurnAndWait,
  startC2Replay,
  stopC2Replay,
  type C2Diff,
  type C2Fixture,
  type C2ReplayResult,
} from "./replay-c2.mjs";
import { readSessionColumn } from "./probe-c2-support.mjs";

/** 失败步重建（probe-c2-compact-failure.mts 的函数步语义，静态化到请求下标）。 */
function failureScript(mode: "tool-call" | "empty"): MockStep[] {
  const failureStep: MockStep =
    mode === "empty"
      ? { toolCalls: [] }
      : { toolCalls: [{ name: "Read", input: { file_path: "not-a-real-file.txt" } }] };
  return [{ text: "normal turn answer" }, { text: "normal turn answer" }, failureStep];
}

/** 夹具剧本 → mock 剧本（失败夹具的函数步不在 JSON 里，按探针同语义重建）。 */
function mockScriptFor(fixture: C2Fixture): MockStep[] {
  if (fixture.case === "compact-provider-tool-call") return failureScript("tool-call");
  if (fixture.case === "compact-provider-empty") return failureScript("empty");
  return (fixture.input.script ?? []) as MockStep[];
}

// ---------------------------------------------------------------------------
// manual compact（compact-manual / compact-no-instructions 共用流程）
// ---------------------------------------------------------------------------

async function replayManualCompact(
  fixture: C2Fixture,
  options: { turnsBeforeCompact: number; withExpectedRevision: boolean; postTurn: boolean },
): Promise<C2ReplayResult> {
  const diffs: C2Diff[] = [];
  const notes: string[] = [];
  const run = await startC2Replay(fixture, { mockScript: mockScriptFor(fixture) });
  try {
    const prompts = (fixture.input.prompts as string[]) ?? [];
    for (const prompt of prompts.slice(0, options.turnsBeforeCompact)) {
      await sendTurnAndWait(run, prompt);
    }
    const expectedRevision = options.withExpectedRevision ? await readStateRevision(run) : undefined;
    const timeUpdatedBefore = readSessionColumn(run.dbPath, "time_updated");
    const eventsBoundary = run.frames().length;
    const terminalBaseline = run.terminalCount();
    const dbBefore = (await readReplayDb(run)) as DbSnapshot;
    const before = {
      messageCount: rowsOf(dbBefore, "message").length,
      turnUsageCount: rowsOf(dbBefore, "turn_usage").length,
      modelUsageCount: rowsOf(dbBefore, "model_usage").length,
    };
    const inputId = (fixture.input.inputId as string) ?? "input_compact_replay";
    const compactingSamples = new Set<string>();
    const compactPromise = run.session.request("session/compact", {
      sessionId: run.session.sessionId,
      inputId,
      ...(fixture.input.instructions !== undefined ? { instructions: fixture.input.instructions } : {}),
      ...(expectedRevision !== undefined ? { expectedRevision } : {}),
    });
    // 受理窗口的 `time_compacting` 采样（§1.3：观察值恒 null；Rust 保持 null 是登记行为）。
    let settled = false;
    const poll = (async () => {
      while (!settled) {
        compactingSamples.add(String(readSessionColumn(run.dbPath, "time_compacting") ?? "null"));
        await delay(2);
      }
    })();
    let result: JsonObject;
    try {
      result = (await compactPromise) as JsonObject;
      await run.waitForTerminals(terminalBaseline + 1);
    } finally {
      settled = true;
      await poll;
    }
    compactingSamples.add(String(readSessionColumn(run.dbPath, "time_compacting") ?? "null"));
    diffs.push(...diffDeep("rows", "timeCompactingDistinctValues", [...compactingSamples].sort(), ["null"]));
    const timeUpdatedAfter = readSessionColumn(run.dbPath, "time_updated");
    diffs.push(...diffDeep("rows", "timeUpdated.increased", Number(timeUpdatedAfter) > Number(timeUpdatedBefore), true));
    // 受理应答（echo + 形状 + 快照可比面）。
    const returned = ((result.compact as JsonObject | undefined)?.inputId as string) ?? null;
    diffs.push(...diffDeep("response", "inputIdEcho.equal", returned === inputId, true));
    diffs.push(...acceptedResponseDiffs(result, fixture.expect.compactResponse as JsonObject, "compactResponse", (value: unknown) => normalizeDeep(value, run.context)));
    // 事件（登记缺省同删后逐条）。
    diffs.push(...diffEventStream(run.normalizedEvents(eventsBoundary), run.normalizedFixtureEvents((fixture.expect.events as JsonObject[]) ?? [])));
    // compact 增量行 + session 行（title 三列折叠）。
    const dbAfterCompact = (await readReplayDb(run)) as DbSnapshot;
    const readBefore = (fixture.expect.readBefore as JsonObject | undefined) ?? {};
    const expectedBeforeMessageCount = Number(readBefore.messageCount ?? rowsOf((fixture.expect.dbAfterCompact as DbSnapshot) ?? {}, "message").length - 2);
    diffs.push(...diffCompactAddedRows(before, dbAfterCompact, (fixture.expect.dbAfterCompact ?? {}) as DbSnapshot, expectedBeforeMessageCount));
    diffs.push(...diffDbRows("dbAfterCompact.session", foldSessionRow(rowsOf(dbAfterCompact, "session")[0] ?? null), foldSessionRow(rowsOf((fixture.expect.dbAfterCompact ?? {}) as DbSnapshot, "session")[0] ?? null)));
    if (options.postTurn) {
      await sendTurnAndWait(run, prompts.at(-1) as string);
      const dbAfterNextTurn = (await readReplayDb(run)) as DbSnapshot;
      // post turn 是 prompt turn：行形状属 C1 时代的未对齐面（登记），这里只比行数增量。
      diffs.push(...diffDeep("rows", "dbAfterNextTurn.messageDelta", rowsOf(dbAfterNextTurn, "message").length - rowsOf(dbAfterCompact, "message").length, 2));
      diffs.push(...diffDeep("rows", "dbAfterNextTurn.partDelta", rowsOf(dbAfterNextTurn, "part").length - rowsOf(dbAfterCompact, "part").length, 4));
      diffs.push(...diffDeep("rows", "dbAfterNextTurn.turnUsageDelta", rowsOf(dbAfterNextTurn, "turn_usage").length - rowsOf(dbAfterCompact, "turn_usage").length, 1));
    }
    // mock 请求路径（isolation 面的观察值；夹具在全部 turn 之后记录）。
    diffs.push(...diffDeep("rows", "isolation.mockRequestPaths", [...run.mockRequestPaths], ((fixture.expect.isolation as JsonObject)?.mockRequestPaths as string[]) ?? []));
    if (run.session.invalidLines.length > 0) {
      throw new Error(`replay-c2：stdout 解析失败行：${run.session.invalidLines[0]}`);
    }
    return { case: fixture.case, diffs, notes, stderr: run.session.stderr };
  } finally {
    await stopC2Replay(run);
  }
}

function delay(ms: number): Promise<void> {
  return new Promise((resolvePromise) => {
    setTimeout(resolvePromise, ms);
  });
}

// ---------------------------------------------------------------------------
// expected-revision 冲突（§1.1：-32009、零事件、逐表行数稳定）
// ---------------------------------------------------------------------------

async function replayExpectedRevision(fixture: C2Fixture): Promise<C2ReplayResult> {
  const diffs: C2Diff[] = [];
  const notes: string[] = [];
  const run = await startC2Replay(fixture, { mockScript: mockScriptFor(fixture) });
  try {
    const prompts = (fixture.input.prompts as string[]) ?? [];
    for (const prompt of prompts) await sendTurnAndWait(run, prompt);
    const revision = await readStateRevision(run);
    const requested = revision + 1000;
    const dbBefore = (await readReplayDb(run)) as DbSnapshot;
    const eventsBoundary = run.frames().length;
    let rejected: JsonObject | null = null;
    try {
      const result = await run.session.request("session/compact", {
        sessionId: run.session.sessionId,
        expectedRevision: requested,
      });
      diffs.push({
        layer: "response",
        path: "rejected",
        message: "过期 expectedRevision 应该被拒绝",
        actual: result,
        expected: null,
      });
    } catch (error) {
      rejected = parseRpcError(error);
    }
    if (rejected === null) {
      diffs.push({ layer: "response", path: "rejected", message: "没有解析到 JSON-RPC 错误", actual: null, expected: { code: -32009 } });
    } else {
      diffs.push(...diffDeep("response", "rejected.code", rejected.code, -32009));
      diffs.push(...diffDeep("response", "rejected.message", rejected.message, "Session state revision mismatch"));
      const data = (rejected.data ?? null) as JsonObject | null;
      diffs.push(...diffDeep("response", "rejected.data.actualRevision", data?.actualRevision, revision));
      diffs.push(...diffDeep("response", "rejected.data.expectedRevision", data?.expectedRevision, requested));
    }
    notes.push(
      `expectedRevision 语义按回放侧事实断言：actualRevision=${revision}（Rust revision 起点，§4.5 登记）`,
    );
    diffs.push(...diffEventStream(run.normalizedEvents(eventsBoundary), []));
    const dbAfter = (await readReplayDb(run)) as DbSnapshot;
    diffs.push(...diffDeep("rows", "allTableRowCountsStable", dbAfter, dbBefore));
    // 前置状态的**列定义**（schema；T7 的对齐面）。行数/行内容属 C1 时代未对齐面
    //（会话 timeline 行、per-turn model_usage 等——见文件头登记），不进比较门。
    diffs.push(...diffTableShapes("dbBeforeRows", dbBefore, (fixture.expect.dbBeforeRows ?? {}) as DbSnapshot));
    return { case: fixture.case, diffs, notes, stderr: run.session.stderr };
  } finally {
    await stopC2Replay(run);
  }
}

// ---------------------------------------------------------------------------
// provider 失败（§1.8：accepted → 异步 turn.failed → 失败行）
// ---------------------------------------------------------------------------

async function replayProviderFailure(fixture: C2Fixture, mode: "tool-call" | "empty"): Promise<C2ReplayResult> {
  const diffs: C2Diff[] = [];
  const run = await startC2Replay(fixture, { mockScript: failureScript(mode) });
  try {
    const prompts = (fixture.input.prompts as string[]) ?? [];
    for (const prompt of prompts) await sendTurnAndWait(run, prompt);
    const eventsBoundary = run.frames().length;
    const dbBefore = (await readReplayDb(run)) as DbSnapshot;
    const beforeMessageCount = rowsOf(dbBefore, "message").length;
    const inputId = `input_compact_${mode}`;
    const result = (await run.session.request("session/compact", {
      sessionId: run.session.sessionId,
      inputId,
    })) as JsonObject;
    const baseline = run.terminalCount();
    await run.waitForTerminals(baseline + 1);
    // accepted 应答形状（失败路径同样是异步受理，§1.8）。
    diffs.push(...acceptedResponseDiffs(result, fixture.expect.compactResponse as JsonObject, "compactResponse", (value: unknown) => normalizeDeep(value, run.context)));
    // 事件（含 turn.failed 载荷逐键）。
    diffs.push(...diffEventStream(run.normalizedEvents(eventsBoundary), run.normalizedFixtureEvents((fixture.expect.events as JsonObject[]) ?? [])));
    // compact 增量行（失败行：failed timeline 消息 + 2 个 part + usage 行）。
    const dbAfter = (await readReplayDb(run)) as DbSnapshot;
    const expectedAfter = (fixture.expect.dbAfterRows ?? {}) as DbSnapshot;
    const expectedBeforeMessageCount = Number(expectedAfter.message?.rowCount ?? 0) - 1;
    diffs.push(...diffCompactAddedRows({ messageCount: beforeMessageCount, turnUsageCount: rowsOf(dbBefore, "turn_usage").length, modelUsageCount: rowsOf(dbBefore, "model_usage").length }, dbAfter, expectedAfter, expectedBeforeMessageCount));
    // 计数关系（§1.8 的共同观察：不写 compact_summary，failed timeline 行 1 条）；
    // message 总行数含 oracle 的会话 timeline 行（C1 时代未对齐面，登记）——只比 C2 拥有的计数。
    const counts = summaryCounts(dbAfter);
    const expectCounts = (fixture.expect.dbAfter ?? {}) as JsonObject;
    diffs.push(...diffDeep("rows", "dbAfter.compactSummaryMessages", counts.compactSummaryMessages, expectCounts.compactSummaryMessages));
    diffs.push(...diffDeep("rows", "dbAfter.compactionParts", counts.compactionParts, expectCounts.compactionParts));
    diffs.push(...diffDeep("rows", "dbAfter.failedTimelineMessages", counts.failedTimelineMessages, expectCounts.failedTimelineMessages));
    return { case: fixture.case, diffs, notes: [], stderr: run.session.stderr };
  } finally {
    await stopC2Replay(run);
  }
}

// ---------------------------------------------------------------------------
// same-tick 双 compact（§1.6：时序相关，两种结果都登记）
// ---------------------------------------------------------------------------

const SKIPPED_RESPONSE = "Context is up to date; no compression needed";

/** skipped 终态的 tokenCount 是本地估算（§1.9 同族登记），两侧同删。 */
function foldSkippedTokenCount(event: { type: string; payload: unknown } | JsonObject): {
  type: string;
  payload: unknown;
} {
  const record = event as { type?: string; payload?: JsonObject };
  if (
    record.type === "turn.completed" &&
    (record.payload as JsonObject | undefined)?.response === SKIPPED_RESPONSE
  ) {
    const payload = { ...(record.payload as JsonObject) };
    delete payload.tokenCount;
    return { type: "turn.completed", payload };
  }
  return { type: String(event.type), payload: event.payload };
}

/**
 * same-tick 的事件期望：夹具按 trial 记录（trial 1–3 是 already_running 形状、
 * trial 4 是 skipped 形状），本 harness 只跑一次 trial——按观察到的 outcome 取
 * 对应 trial 的录制事件（登记缺省同删后比较 turn.started/turn.completed 载荷）。
 */
function skippedAwareExpectedEvents(fixture: C2Fixture, secondState: unknown): JsonObject[] {
  const trials = (fixture.expect.trials as JsonObject[]) ?? [];
  const trial = secondState === "accepted" ? trials.at(-1) : trials[0];
  const statusEvents = ((trial?.statusEvents ?? []) as JsonObject[]).filter(
    (event) => !C2_ABSENT_FOR_SAME_TICK.includes(String(event.type)),
  );
  return statusEvents;
}

const C2_ABSENT_FOR_SAME_TICK = ["session.updated", "session.titleUpdated", "streamRecovery.updated"];

async function replaySameTick(fixture: C2Fixture): Promise<C2ReplayResult> {
  const diffs: C2Diff[] = [];
  const notes: string[] = [];
  const run = await startC2Replay(fixture, { mockScript: mockScriptFor(fixture) });
  try {
    const prompts = (fixture.input.prompts as string[]) ?? [];
    for (const prompt of prompts) await sendTurnAndWait(run, prompt);
    const eventsBoundary = run.frames().length;
    const terminalBaseline = run.terminalCount();
    const inputIds = ["input_compact_trial_1_1", "input_compact_trial_1_2"];
    // 同 tick 连发（与 probe-c2-compact-race 同驱动）：结果时序相关（§1.6）。
    const first = run.session.request("session/compact", {
      sessionId: run.session.sessionId,
      inputId: inputIds[0],
    });
    const second = run.session.request("session/compact", {
      sessionId: run.session.sessionId,
      inputId: inputIds[1],
    });
    const results = await Promise.all([first, second]);
    const states = results.map(
      (result) => ((result as JsonObject).compact as JsonObject | undefined)?.state,
    );
    if (states[0] !== "accepted") {
      diffs.push({ layer: "response", path: "firstCompact.state", message: "第一单恒 accepted", actual: states[0], expected: "accepted" });
    }
    diffs.push(
      ...diffDeep(
        "response",
        "inputIdEcho",
        results.map((result) => ((result as JsonObject).compact as JsonObject | undefined)?.inputId),
        inputIds,
      ),
    );
    const outcome = states[1] === "already_running" ? "accepted+already_running" : states[1] === "accepted" ? "accepted+accepted" : `unexpected:${String(states[1])}`;
    notes.push(`same-tick outcome（时序相关，§1.6 两分支都是登记结果）: ${outcome}`);
    if (states[1] !== "already_running" && states[1] !== "accepted") {
      diffs.push({ layer: "response", path: "secondCompact.state", message: "第二单只能是登记的两种形状", actual: states[1], expected: ["already_running", "accepted"] });
    }
    const acceptedCount = states.filter((state) => state === "accepted").length;
    await run.waitForTerminals(terminalBaseline + acceptedCount);
    const db = (await readReplayDb(run)) as DbSnapshot;
    // trial 计数形状（probe-c2-compact-race 的 compactionCounts；不含 message 总数——
    // 会话 timeline 行是 C1 时代未对齐面）。
    const counts = {
      compactionParts: rowsOf(db, "part").filter((row) => ((row.data as JsonObject)?.type as string) === "compaction").length,
      compactSummaryMessages: rowsOf(db, "message").filter((row) => (((row.data as JsonObject)?.semantics as JsonObject | undefined)?.kind as string) === "compact_summary").length,
      skippedTimelineMessages: rowsOf(db, "message").filter((row) => ((row.data as JsonObject)?.finish as string) === "skipped").length,
    };
    // 两种登记结果的 DB 计数（trial 1 / trial 4 的 dbCounts）。
    const trials = (fixture.expect.trials as JsonObject[]) ?? [];
    const expectTrial = states[1] === "already_running" ? trials[0] : trials.at(-1);
    diffs.push(...diffDeep("rows", "dbCounts", counts, expectTrial?.dbCounts ?? null));
    if (states[1] === "accepted") {
      // skipped 行逐字段（trial 4 的 dbSkippedRows：1 条 skipped 消息 + 1 个 skipped compaction part）。
      const expectSkipped = (expectTrial?.dbSkippedRows ?? {}) as JsonObject;
      const actualSkipped = {
        messages: rowsOf(db, "message").filter((row) => ((row.data as JsonObject)?.finish as string) === "skipped").map(foldRowPosition),
        compactionParts: rowsOf(db, "part").filter((row) => ((row.data as JsonObject)?.timelineStatus as string) === "skipped").map(foldRowPosition),
      };
      diffs.push(...diffDbRows("dbSkippedRows.messages", actualSkipped.messages, ((expectSkipped.messages ?? []) as JsonObject[]).map(foldRowPosition)));
      diffs.push(...diffDbRows("dbSkippedRows.compactionParts", actualSkipped.compactionParts, ((expectSkipped.compactionParts ?? []) as JsonObject[]).map(foldRowPosition)));
    }
    // already_running 分支的摘要行形状由 compact-no-instructions 用例覆盖（同剧本同消耗）；
    // trial 录制面只有 dbCounts + dbSkippedRows（见 probe-c2-compact-race 的投影），不重复。
    // skipped 终态的 turn.completed 文案（§1.6 观察值）在事件层比较里逐字断言；
    // skipped 的 `tokenCount` 是本地估算（§1.9 登记的「oracle 公式未观察」同族——
    // Rust 113 vs oracle 2843），两侧同删；其余载荷（response/cacheStats/…）逐字比。
    diffs.push(
      ...diffEventStream(
        run.normalizedEvents(eventsBoundary).map(foldSkippedTokenCount),
        skippedAwareExpectedEvents(fixture, states[1]).map(foldSkippedTokenCount),
      ),
    );
    return { case: fixture.case, diffs, notes, stderr: run.session.stderr };
  } finally {
    await stopC2Replay(run);
  }
}

// ---------------------------------------------------------------------------
// 注册表
// ---------------------------------------------------------------------------

export function registerCompactCases(registry: Record<string, (fixture: C2Fixture) => Promise<C2ReplayResult>>): void {
  registry["compact-manual"] = (fixture) =>
    replayManualCompact(fixture, { turnsBeforeCompact: ((fixture.input.prompts as string[]) ?? []).length - 1, withExpectedRevision: true, postTurn: true });
  registry["compact-no-instructions"] = (fixture) =>
    replayManualCompact(fixture, { turnsBeforeCompact: ((fixture.input.prompts as string[]) ?? []).length, withExpectedRevision: false, postTurn: false });
  registry["compact-expected-revision"] = replayExpectedRevision;
  registry["compact-same-tick"] = replaySameTick;
  registry["compact-provider-tool-call"] = (fixture) => replayProviderFailure(fixture, "tool-call");
  registry["compact-provider-empty"] = (fixture) => replayProviderFailure(fixture, "empty");
}
