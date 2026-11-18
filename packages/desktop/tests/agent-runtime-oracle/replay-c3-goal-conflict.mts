// C3 Task 5：goal 冲突/循环夹具回放的 case drivers（比较面见 replay-c3.mts 文件头）。
//
// - `goal-replace-revision-conflict`：-32009 冲突零副作用、正确 revision 的 replace、
//   运行中管理拒绝（-32010）、参数校验（-32602 / Usage 文案）。
// - `goal-continuation-loop`（有界）：verifier 持续 passed:false 的自动续跑——夹具
//   观察了 40 轮，回放只跑**确定性子集**（首轮事件切片 + 前 5 个 goalIteration +
//   pause 形状 + read 的验证历史形状）；绝对计数（turnCount/eventSeq/tokensUsed）
//   与轮次相关，不比（Rust 单测 `multiple_goal_iterations...` 断言多迭代账务）。
import type { JsonObject } from "./session-lifecycle-fixtures.mjs";
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
} from "./replay-c3.mjs";
import {
  foldSnapshotProjection,
  isCancelledTerminal,
  isGoalContinuation,
  isTargetStatus,
  staticScript,
  verifierAux,
} from "./replay-c3-goal.mjs";

// ---------------------------------------------------------------------------
// Case：replace + expectedRevision 冲突 + 运行中拒绝 + 参数校验
// ---------------------------------------------------------------------------

async function replayReplaceRevision(fixture: C3Fixture): Promise<C3ReplayResult> {
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
    await callGoal(run, { action: "set", objective: "Goal before replace", inputId: "goal_r_set" });
    await run.waitForEvent(start, isGoalContinuation, "replace 前续跑开始");
    await callGoal(run, { action: "pause", inputId: "goal_r_pause" });
    await run.waitForEvent(start, isCancelledTerminal, "pause 取消");
    const before = await run.readSessionNormalized();
    const revision = Number((before.runtime as JsonObject | undefined)?.stateRevision ?? 0);
    const beforeEventCount = run.frames().length;
    const beforeDb = run.readTables();
    const conflict = await callGoal(run, {
      action: "set",
      objective: "must not apply",
      inputId: "goal_r_conflict",
      expectedRevision: Math.max(0, revision - 1),
    });
    await delay(300);
    const conflictEvents = run.frames().slice(beforeEventCount);
    const conflictDb = run.readTables();
    const conflictRead = await run.readSessionNormalized();
    const conflictTarget = ((conflictDb.session_target as JsonObject | undefined)?.rows ??
      []) as JsonObject[];
    const conflictExpected = expect.conflict as JsonObject;
    diffs.push(...diffC3Value("response", "conflict.error", conflict.ok ? null : conflict.error, conflictExpected.error ?? null));
    diffs.push(...diffC3Value("response", "conflict.result", conflict.ok ? conflict.result : null, conflictExpected.result ?? null));
    diffs.push(...diffC3Value("rows", "conflict.sideEffects", {
      newEvents: conflictEvents.length,
      stateRevisionBefore: revision,
      stateRevisionAfter: Number((conflictRead.runtime as JsonObject | undefined)?.stateRevision ?? 0),
      targetObjectiveBefore: ((before.session as JsonObject | undefined)?.target as JsonObject)?.objective ?? null,
      targetObjectiveAfter: (conflictTarget[0] as JsonObject | undefined)?.objective ?? null,
      targetRowCountBefore: (beforeDb.session_target as JsonObject | undefined)?.rowCount ?? null,
      targetRowCountAfter: (conflictDb.session_target as JsonObject | undefined)?.rowCount ?? null,
    }, conflictExpected.sideEffects ?? null));
    // 正确 revision 的 replace → 续跑 → 运行中拒绝 → 完成。
    const replaceStart = run.frames().length;
    const replace = await callGoal(run, {
      action: "replace",
      objective: "Goal after replace",
      inputId: "goal_r_replace",
      expectedRevision: revision,
    });
    await run.waitForEvent(replaceStart, isGoalContinuation, "replace 后续跑开始");
    const whileRunning = await callGoal(run, { action: "set", objective: "set while running", inputId: "goal_r_running" });
    const replaceWhileRunning = await callGoal(run, { action: "replace", objective: "replace while running", inputId: "goal_r_replace_running" });
    const resumeWhileRunning = await callGoal(run, { action: "resume", inputId: "goal_r_resume_running" });
    const clearWhileRunning = await callGoal(run, { action: "clear", inputId: "goal_r_clear_running" });
    await run.waitForEvent(replaceStart, isTargetStatus("complete"), "replace 后 complete");
    await delay(200);
    const after = await run.readSessionNormalized();
    const invalidAction = await callGoal(run, { action: "bogus" });
    const tokenBudget = await callGoal(run, { action: "set", objective: "budget", tokenBudget: 100 });
    const missingObjective = await callGoal(run, { action: "set", inputId: "goal_r_missing" });
    const missingReplaceObjective = await callGoal(run, { action: "replace" });
    const actionShape = (result: Awaited<ReturnType<typeof callGoal>>): JsonObject =>
      result.ok
        ? {
            response: result.result.response ?? null,
            startedTurn: result.result.startedTurn ?? null,
            snapshot: result.result.snapshot !== undefined
              ? foldSnapshotProjection(projectGoalSnapshot(result.result))
              : undefined,
          }
        // callGoal 的 error 已是 normalizeError 输出（{frame:…}），不再二次折叠。
        : { error: result.error };
    const foldExpectedAction = (expected: JsonObject | undefined): JsonObject | undefined => {
      if (expected === undefined) return undefined;
      const folded = { ...expected };
      if (folded.snapshot !== undefined && folded.snapshot !== null) {
        folded.snapshot = foldSnapshotProjection(folded.snapshot as JsonObject);
      }
      return folded;
    };
    diffs.push(...diffC3Value("response", "replace", actionShape(replace), foldExpectedAction(expect.replace as JsonObject) ?? null));
    for (const [key, value] of [
      ["setWhileRunning", whileRunning],
      ["replaceWhileRunning", replaceWhileRunning],
      ["resumeWhileRunning", resumeWhileRunning],
      ["clearWhileRunning", clearWhileRunning],
    ] as const) {
      diffs.push(...diffC3Value("response", key, actionShape(value), (expect[key] as JsonObject) ?? null));
    }
    // -32602 参数校验：Rust 的文案/data 是 serde 严格反序列化产物（`session/goal
    // 参数不合法: unknown variant …`），oracle 是 ZodError 镜像——code 一致、文案与
    // data 形状不同（§C3 新登记，不为此改协议错误面）：只比 code。
    for (const [key, value] of [
      ["invalidAction", invalidAction],
      ["tokenBudget", tokenBudget],
    ] as const) {
      const codeOf = (result: Awaited<ReturnType<typeof callGoal>>): number | null =>
        result.ok
          ? null
          : (((result.error as JsonObject).frame as JsonObject | undefined)?.code as number) ?? null;
      diffs.push(...diffC3Value("response", `${key}.errorCode`, codeOf(value), ((((expect[key] as JsonObject)?.error as JsonObject | undefined)?.frame as JsonObject | undefined)?.code as number) ?? null));
    }
    for (const [key, value, expectedKey] of [
      ["missingObjective", missingObjective, "missingObjective"],
      ["missingReplaceObjective", missingReplaceObjective, "missingReplaceObjective"],
    ] as const) {
      const projected: JsonObject = value.ok
        ? { response: value.result.response ?? null, startedTurn: value.result.startedTurn ?? null }
        : { error: value.error };
      diffs.push(...diffC3Value("response", key, projected, (expect[expectedKey] as JsonObject) ?? null));
    }
    const foldRead = (read: JsonObject): JsonObject => {
      const folded = { ...read };
      delete folded.eventSeq;
      delete folded.stateRevision;
      return folded;
    };
    diffs.push(...diffC3Value("response", "readBefore", foldRead(projectGoalRead(before)), foldRead(expect.readBefore as JsonObject)));
    diffs.push(...diffC3Value("response", "readAfter", foldRead(projectGoalRead(after)), foldRead(expect.readAfter as JsonObject)));
    diffs.push(...diffEventStream(run.normalizedActualEvents(start, GOAL_EVENT_SCOPE), run.normalizedFixtureEvents((expect.events as JsonObject[]) ?? [], GOAL_EVENT_SCOPE)));
    // replace 覆盖已有目标时 oracle 多写一条 goal_state_change 行（正文未观察，
    // §C3 登记不伪造）；goal_state_change 行的存在性由 goal-pause-resume-clear 覆盖——
    // 本 case 两侧同删该类行后逐行比。actual 侧的 source 从行数据原地重读
    //（Rust 行的 source 在 semantics.source，oracle 在 data.source——§C3 登记的
    // 行形状差），使两侧的 goal_state_change 都可识别。
    const foldSummary = (rows: JsonObject[]): JsonObject[] =>
      rows
        .filter((row) => row.source !== "goal_state_change" && row.semanticsKind !== "timeline_event")
        .map((row) => ({ ...row, sequence: "<row-position>" }));
    const actualRows = ((run.readTables().message as JsonObject | undefined)?.rows ?? []) as JsonObject[];
    const actualSummary = actualRows.map((row) => {
      const data = (row.data ?? {}) as JsonObject;
      const semantics = (data.semantics ?? {}) as JsonObject;
      return {
        sequence: row.sequence ?? null,
        role: data.role ?? null,
        finish: data.finish ?? null,
        semanticsKind: semantics.kind ?? null,
        source: (semantics.source as string | undefined) ?? (data.source as string | undefined) ?? null,
        parentID: data.parentID ?? null,
      };
    });
    diffs.push(...diffC3Value("db", "messageSummary", foldSummary(actualSummary), foldSummary(((expect.db as JsonObject).messageSummary ?? []) as JsonObject[])));
    const withoutSummary = (projection: JsonObject): JsonObject => {
      const folded = { ...projection };
      delete folded.messageSummary;
      return folded;
    };
    diffs.push(...diffGoalDb("db", withoutSummary(projectGoalDb(run.readTables())), withoutSummary(expect.db as JsonObject)));
    if (run.session.invalidLines.length > 0) {
      throw new Error(`replay-c3：stdout 解析失败行：${run.session.invalidLines[0]}`);
    }
    notes.push(
      "-32602 参数校验文案不比（serde vs ZodError 镜像，§C3 登记）；replace 的 goal_state_change 行缺口登记（§C3），本 case 两侧同删该类行",
    );
    return { case: fixture.case, diffs, notes, stderr: run.session.stderr };
  } finally {
    await stopC3Replay(run);
  }
}

// ---------------------------------------------------------------------------
// Case：continuation loop（有界 3 轮 + pause；确定性子集）
// ---------------------------------------------------------------------------

const LOOP_TURNS = 3;

async function replayContinuationLoop(fixture: C3Fixture): Promise<C3ReplayResult> {
  const diffs: C3Diff[] = [];
  const notes: string[] = [];
  const run = await startC3Replay({
    fixture,
    mockScript: staticScript(fixture),
    auxiliaryResponse: verifierAux(false),
  });
  try {
    const expect = fixture.expect;
    const start = run.frames().length;
    await callGoal(run, { action: "set", objective: "Loop goal", inputId: "goal_loop_set" });
    // 等第 LOOP_TURNS 个**不同的** continuation turn.started（verifier 持续
    // passed:false；与 probe 的循环计数同口径——同一个事件不能重复计数）。
    const deadline = Date.now() + 120_000;
    // 夹具的 firstIterations 是验证事件序列的前 5 个（v1s,v1c,v2s,v2c,v3s）——
    // 第 3 轮的 verifier started 必须在 pause 前已发出，轮询以验证事件计数为准。
    const verificationCount = (): number =>
      run
        .frames()
        .slice(start)
        .filter((event) => {
          const payload = (event.payload ?? {}) as JsonObject;
          return event.type === "session.updated" && payload.verificationId !== undefined;
        }).length;
    while (true) {
      const starts = run
        .frames()
        .slice(start)
        .filter((event) => {
          const payload = (event.payload ?? {}) as JsonObject;
          return event.type === "turn.started" && payload.inputSource === "goal-continuation";
        }).length;
      if (starts >= LOOP_TURNS && verificationCount() >= 2 * LOOP_TURNS - 1) break;
      if (Date.now() > deadline) {
        throw new Error(
          `replay-c3：loop 在 120s 内只观察到 ${starts} 个 continuation turn / ${verificationCount()} 个验证事件`,
        );
      }
      await delay(20);
    }
    const pause = await callGoal(run, { action: "pause", inputId: "goal_loop_pause" });
    await run.waitForEvent(
      start,
      (event) => isTargetStatus("paused")(event) || isCancelledTerminal(event),
      "loop pause 生效",
    );
    await delay(500);
    // 确定性子集 1：首轮事件切片（夹具 firstEvents = 前 12 条原始事件）。
    const expectedFirst = run.normalizedFixtureEvents((expect.firstEvents as JsonObject[]) ?? [], GOAL_EVENT_SCOPE);
    const actualFirst = run.normalizedActualEvents(start, GOAL_EVENT_SCOPE).slice(0, expectedFirst.length);
    diffs.push(...diffEventStream(actualFirst, expectedFirst));
    // 确定性子集 2：验证事件的 goalIteration 序列前 5 个（started/completed 成对）。
    const iterations = run
      .frames()
      .slice(start)
      .filter((event) => {
        const payload = (event.payload ?? {}) as JsonObject;
        return event.type === "session.updated" && payload.verificationId !== undefined;
      })
      .map((event) => ((event.payload ?? {}) as JsonObject).goalIteration)
      .filter((value): value is number => typeof value === "number")
      .slice(0, 5);
    diffs.push(...diffC3Value("rows", "firstIterations", iterations, expect.firstIterations ?? null));
    // 确定性子集 3：pause 形状与 read 的验证历史形状（绝对计数不比）。
    diffs.push(...diffC3Value("response", "pause", {
      response: pause.ok ? pause.result.response ?? null : null,
      startedTurn: pause.ok ? pause.result.startedTurn ?? null : null,
    }, expect.pause as JsonObject));
    const read = projectGoalRead(await run.readSessionNormalized());
    const readExpected = expect.read as JsonObject;
    // target 只比轮次无关字段（status/objective/summaryTitle；tokensUsed 与轮数线性
    // 相关，绝对值随有界子集不同）。
    const target = (read.target ?? {}) as JsonObject;
    const expectedTarget = (readExpected.target ?? {}) as JsonObject;
    diffs.push(...diffC3Value("response", "read.target.status", target.status ?? null, expectedTarget.status ?? null));
    diffs.push(...diffC3Value("response", "read.target.objective", target.objective ?? null, expectedTarget.objective ?? null));
    diffs.push(...diffC3Value("response", "read.sessionStatus", read.sessionStatus ?? null, readExpected.sessionStatus ?? null));
    diffs.push(...diffC3Value("response", "read.projectionStatus", read.projectionStatus ?? null, readExpected.projectionStatus ?? null));
    // goalVerificationsLast / timelineFirst 由投影数组派生（夹具的 read 是
    // caseContinuationLoop 的自定义投影；projectGoalRead 输出的是完整数组）。
    const verifications = (read.goalVerifications ?? []) as JsonObject[];
    const timeline = (read.goalVerificationTimeline ?? []) as JsonObject[];
    // 窗口计数：夹具观察 goalVerificationCount:2（§1.11「length:2」）。最后一项
    // 不逐字比——oracle 的窗口含最近一次 cancelled 验证（夹具值是 cancelled
    // reason），Rust 只保留 completed（§1.11 已登记差异）；completed 项的形状由
    // timelineFirst 逐字覆盖。
    diffs.push(...diffC3Value("response", "read.goalVerificationCount", verifications.length, readExpected.goalVerificationCount ?? null));
    diffs.push(...diffC3Value("response", "read.timelineFirst", timeline[0] ?? null, readExpected.timelineFirst ?? null));
    notes.push(
      `有界回放：夹具观察 40 轮，回放只跑 ${LOOP_TURNS} 轮确定性子集——绝对计数` +
        "（observedContinuationTurns/observedTerminals/turnCount/eventSeq/tokensUsed/timelineCount）不比；" +
        "多迭代账务与 rc 序数由 crates/agent-server/tests/goal.rs 的多迭代用例断言（§C3 登记）",
    );
    if (run.session.invalidLines.length > 0) {
      throw new Error(`replay-c3：stdout 解析失败行：${run.session.invalidLines[0]}`);
    }
    notes.push(
      "-32602 参数校验文案不比（serde vs ZodError 镜像，§C3 登记）；replace 的 goal_state_change 行缺口登记（§C3），本 case 两侧同删该类行",
    );
    return { case: fixture.case, diffs, notes, stderr: run.session.stderr };
  } finally {
    await stopC3Replay(run);
  }
}

export function registerC3GoalConflictCases(registry: Record<string, (fixture: C3Fixture) => Promise<C3ReplayResult>>): void {
  registry["goal-replace-revision-conflict"] = replayReplaceRevision;
  registry["goal-continuation-loop"] = replayContinuationLoop;
}

export { loadC3Fixture };
