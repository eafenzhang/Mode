// C3 Task 5：goal 夹具回放的 case drivers（`replay-c3.mts` 的比较半边）。
//
// 驱动序与 `probe-c3-goal.mts` 逐 case 相同（同一驾驶步，换 Rust server 执行）；比较面
// 见 `replay-c3.mts` 文件头与各 case 注记。goal 用例的事件范围 = GOAL_EVENT_SCOPE
// （保留 titleUpdated；删 model_request_completed——Rust 只在 retry 收口产出）。
//
// 登记差异（比较面之外，`tauri-agent-runtime.md` §C3）：verifier 请求体不比（提示词
// clean-room 自撰 §1.11、system 段数 §1.5）；`read`/快照的 `eventSeq`/`stateRevision`
// 绝对值不比（起点事实）；goal-continuation 提示词 `input` 两侧同删。
import type { JsonObject } from "./session-lifecycle-fixtures.mjs";
import type { MockStep } from "./mock-provider.mjs";
import {
  callGoal,
  projectGoalDb,
  projectGoalRead,
  projectGoalSnapshot,
} from "./probe-c3-support.mjs";
import { delay } from "./probe-c2-support.mjs";
import {
  diffC3Value,
  diffEventStream,
  diffGoalDb,
  GOAL_EVENT_SCOPE,
  loadC3Fixture,
  startC3Replay,
  stopC3Replay,
  type C3Diff,
  type C3Fixture,
  type C3ReplayResult,
  type C3Run,
} from "./replay-c3.mjs";

const SEED = { "hello.txt": "hello\n" };
const VERIFIER_PASS = JSON.stringify({ passed: true, reason: "goal verified", nextAction: null });
const VERIFIER_FAIL = JSON.stringify({ passed: false, reason: "not yet", nextAction: "continue" });

/**
 * verifier 应答注入：goal 场景里唯一无工具请求就是 verifier（标题生成关闭）。
 * Rust 的 verifier 请求是流式消费（`stream:true` + 0 tools；oracle 非流式——
 * 形状差异登记 §C3），`sse:true` 让 mock 回 SSE 而不是 JSON 报文。
 */
function verifierAux(passed: boolean) {
  return () => ({ text: passed ? VERIFIER_PASS : VERIFIER_FAIL, sse: true });
}

function isGoalContinuation(event: { type: string; payload: unknown }): boolean {
  const payload = (event.payload ?? {}) as JsonObject;
  return event.type === "turn.started" && payload.inputSource === "goal-continuation";
}

function isTargetStatus(status: string) {
  return (event: { type: string; payload: unknown }): boolean => {
    if (event.type !== "session.updated") return false;
    const payload = (event.payload ?? {}) as JsonObject;
    return (
      payload.action === "status_updated" &&
      ((payload.target as JsonObject | undefined)?.status ?? null) === status
    );
  };
}

function isCancelledTerminal(event: { type: string; payload: unknown }): boolean {
  const payload = (event.payload ?? {}) as JsonObject;
  return event.type === "turn.completed" && payload.resultType === "cancelled";
}

/** 夹具剧本是静态 JSON（[{text, delayMs}]）；step 函数场景在 retry/background drivers。 */
function staticScript(fixture: C3Fixture): MockStep[] {
  return (fixture.input.script ?? []) as MockStep[];
}

/** 应答投影（snapshot 面的可比子集；eventSeq/stateRevision 登记差异两侧同删）。 */
function foldSnapshotProjection(snapshot: JsonObject | null): JsonObject | null {
  if (snapshot === null || typeof snapshot !== "object") return snapshot;
  const folded = { ...snapshot };
  delete folded.eventSeq;
  delete folded.stateRevision;
  return folded;
}

function goalActionDiffs(
  diffs: C3Diff[],
  path: string,
  actual: { ok: true; result: { response?: unknown; startedTurn?: unknown; snapshot?: JsonObject } } | { ok: false; error: JsonObject },
  expected: JsonObject,
  options: { withSnapshot?: boolean } = {},
): void {
  if (actual.ok !== true) {
    diffs.push(...diffC3Value("response", path, { error: actual.error }, expected));
    return;
  }
  const projected: JsonObject = {
    ...(expected.ok !== undefined ? { ok: true } : {}),
    ...(expected.error !== undefined ? { error: null } : {}),
    response: actual.result.response ?? null,
    startedTurn: actual.result.startedTurn ?? null,
    ...(options.withSnapshot === false
      ? {}
      : { snapshot: foldSnapshotProjection(projectGoalSnapshot(actual.result)) }),
  };
  const expectedFolded = { ...(expected as JsonObject) };
  if (expectedFolded.snapshot !== undefined && expectedFolded.snapshot !== null) {
    expectedFolded.snapshot = foldSnapshotProjection(expectedFolded.snapshot as JsonObject);
  }
  diffs.push(...diffC3Value("response", path, projected, expectedFolded));
}

function readDiffs(diffs: C3Diff[], path: string, actual: JsonObject, expected: JsonObject): void {
  const fold = (read: JsonObject): JsonObject => {
    const folded = { ...read };
    delete folded.eventSeq;
    delete folded.stateRevision;
    delete folded.turnCount;
    return folded;
  };
  diffs.push(...diffC3Value("response", path, fold(actual), fold(expected)));
}

// ---------------------------------------------------------------------------
// Case 1：no goal → set → 控制 turn → 续跑 → verifier → complete（夹具全投影）
// ---------------------------------------------------------------------------

async function replayShowSetComplete(fixture: C3Fixture): Promise<C3ReplayResult> {
  const diffs: C3Diff[] = [];
  const notes: string[] = [];
  const run = await startC3Replay({
    fixture,
    mockScript: staticScript(fixture),
    auxiliaryResponse: verifierAux(true),
  });
  try {
    const expect = fixture.expect;
    const start = run.frames().length;
    const showEmpty = await callGoal(run, { action: "show", inputId: "goal_show_empty" });
    const pauseEmpty = await callGoal(run, { action: "pause", inputId: "goal_pause_empty" });
    const resumeEmpty = await callGoal(run, { action: "resume", inputId: "goal_resume_empty" });
    const clearEmpty = await callGoal(run, { action: "clear", inputId: "goal_clear_empty" });
    const setStart = run.frames().length;
    const set = await callGoal(run, {
      action: "set",
      objective: "Ship the C3 goal contract",
      inputId: "goal_set_main",
    });
    const showAfterSet = await callGoal(run, { action: "show", inputId: "goal_show_after_set" });
    await run.waitForEvent(setStart, isGoalContinuation, "goal continuation turn.started");
    await run.waitForEvent(setStart, isTargetStatus("complete"), "goal status complete");
    await delay(300);
    const showComplete = await callGoal(run, { action: "show", inputId: "goal_show_complete" });
    // 应答面（noGoalActions 只有 response/startedTurn；set/show 带快照投影）。
    goalActionDiffs(diffs, "showEmpty", showEmpty, expect.showEmpty as JsonObject);
    goalActionDiffs(diffs, "set", set, expect.set as JsonObject);
    goalActionDiffs(diffs, "showAfterSet", showAfterSet, expect.showAfterSet as JsonObject);
    goalActionDiffs(diffs, "showComplete", showComplete, expect.showComplete as JsonObject);
    diffs.push(
      ...diffC3Value("response", "noGoalActions", {
        pause: pauseEmpty.ok ? { response: pauseEmpty.result.response, startedTurn: pauseEmpty.result.startedTurn } : pauseEmpty,
        resume: resumeEmpty.ok ? { response: resumeEmpty.result.response, startedTurn: resumeEmpty.result.startedTurn } : resumeEmpty,
        clear: clearEmpty.ok ? { response: clearEmpty.result.response, startedTurn: clearEmpty.result.startedTurn } : clearEmpty,
      }, expect.noGoalActions as JsonObject),
    );
    // 事件面（从首帧起全量；登记过滤见文件头）。
    diffs.push(...diffEventStream(run.normalizedActualEvents(start, GOAL_EVENT_SCOPE), run.normalizedFixtureEvents((expect.events as JsonObject[]) ?? [], GOAL_EVENT_SCOPE)));
    // DB 面（session_target/session_entry/turn_usage/model_usage/messageSummary）。
    diffs.push(...diffGoalDb("db", projectGoalDb(run.readTables()), expect.db as JsonObject));
    // 读面（eventSeq/stateRevision/turnCount 不比）。
    readDiffs(diffs, "read", projectGoalRead(await run.readSessionNormalized()), expect.read as JsonObject);
    // verifier 请求体不比（提示词 clean-room §1.11；system 段数 §1.5）——登记。
    notes.push("verifierRequest 不比：提示词自撰（§1.11）+ system 1 段 vs 3 段（§1.5）");
    if (run.session.invalidLines.length > 0) {
      throw new Error(`replay-c3：stdout 解析失败行：${run.session.invalidLines[0]}`);
    }
    return { case: fixture.case, diffs, notes, stderr: run.session.stderr };
  } finally {
    await stopC3Replay(run);
  }
}

// ---------------------------------------------------------------------------
// Case 2：pause（provider 请求前）→ resume → complete → clear
// ---------------------------------------------------------------------------

async function replayPauseResumeClear(fixture: C3Fixture): Promise<C3ReplayResult> {
  const diffs: C3Diff[] = [];
  const notes: string[] = [];
  const run = await startC3Replay({
    fixture,
    mockScript: staticScript(fixture),
    auxiliaryResponse: verifierAux(true),
  });
  try {
    const expect = fixture.expect;
    const start = run.frames().length;
    const set = await callGoal(run, {
      action: "set",
      objective: "Pause resume clear goal",
      inputId: "goal_prc_set",
    });
    await run.waitForEvent(start, isGoalContinuation, "goal continuation turn.started");
    const pause = await callGoal(run, { action: "pause", inputId: "goal_prc_pause" });
    const pauseStart = run.frames().length;
    await run.waitForEvent(pauseStart, isCancelledTerminal, "pause 取消当前 turn");
    await delay(200);
    const readPaused = projectGoalRead(await run.readSessionNormalized());
    const tablesAfterPause = run.readTables();
    const resume = await callGoal(run, { action: "resume", inputId: "goal_prc_resume" });
    const resumeStart = run.frames().length;
    await run.waitForEvent(resumeStart, isTargetStatus("complete"), "resume 后的 verifier complete");
    await delay(200);
    const readComplete = projectGoalRead(await run.readSessionNormalized());
    const clearStart = run.frames().length;
    const clear = await callGoal(run, { action: "clear", inputId: "goal_prc_clear" });
    await delay(200);
    const showAfterClear = await callGoal(run, { action: "show", inputId: "goal_prc_show" });
    const tablesAfterClear = run.readTables();
    goalActionDiffs(diffs, "set", set, expect.set as JsonObject, { withSnapshot: false });
    goalActionDiffs(diffs, "pause", pause, expect.pause as JsonObject);
    goalActionDiffs(diffs, "resume", resume, expect.resume as JsonObject);
    goalActionDiffs(diffs, "clear", clear, expect.clear as JsonObject);
    goalActionDiffs(diffs, "showAfterClear", showAfterClear, expect.showAfterClear as JsonObject, { withSnapshot: false });
    readDiffs(diffs, "readPaused", readPaused, expect.readPaused as JsonObject);
    readDiffs(diffs, "readComplete", readComplete, expect.readComplete as JsonObject);
    // pause 前事件（set → pause 取消）与 clear 之后事件分开比；DB 两份投影。
    diffs.push(...diffEventStream(run.normalizedActualEvents(start, GOAL_EVENT_SCOPE), run.normalizedFixtureEvents((expect.eventsAfterSet as JsonObject[]) ?? [], GOAL_EVENT_SCOPE)));
    diffs.push(...diffEventStream(run.normalizedActualEvents(clearStart, GOAL_EVENT_SCOPE), run.normalizedFixtureEvents((expect.eventsAfterClear as JsonObject[]) ?? [], GOAL_EVENT_SCOPE)));
    const db = expect.db as JsonObject;
    diffs.push(...diffGoalDb("db.afterPause", projectGoalDb(tablesAfterPause), db.afterPause as JsonObject));
    diffs.push(...diffGoalDb("db.afterClear", projectGoalDb(tablesAfterClear), db.afterClear as JsonObject));
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

export function registerC3GoalCases(registry: Record<string, (fixture: C3Fixture) => Promise<C3ReplayResult>>): void {
  registry["goal-show-set-complete"] = replayShowSetComplete;
  registry["goal-pause-resume-clear"] = replayPauseResumeClear;
}

// 供 mixed/restart drivers 复用的公共步骤（不注册 case）。
export { SEED, VERIFIER_PASS, VERIFIER_FAIL, verifierAux, isGoalContinuation, isTargetStatus, isCancelledTerminal, staticScript, goalActionDiffs, readDiffs, foldSnapshotProjection, loadC3Fixture, startC3Replay, stopC3Replay, delay };
export type { C3Run };
