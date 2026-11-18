/* oxlint-disable eslint(max-lines) -- goal 六个 action 的观察矩阵共享同一套夹具投影，拆分会复制事件切片逻辑。 */
// C3 Task 0：`session/goal` 的黑盒观察（show/set/replace/pause/resume/clear）。
//
// 用法：pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c3-goal.mts
//
// 纪律：只运行 oracle、记录它写出的事件/应答/DB 行；不读实现体。夹具里的 `expect`
// 全部由本脚本从运行结果投影，不手写期望值。运行相关数值（目标计时、重试退避）
// 在 `probe-c3-support.mts` 里折叠成稳定 token，原始值只出现在 `observedRun` 注明。
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { JsonObject } from "./session-lifecycle-fixtures.mjs";
import {
  callGoal,
  eventSummary,
  isolationRecord,
  normalizeC3Deep,
  projectEvents,
  projectGoalDb,
  projectGoalRead,
  projectGoalSnapshot,
  readSessionById,
  startC3Harness,
  writeC3Fixture,
  type C3Harness,
} from "./probe-c3-support.mjs";
import { delay } from "./probe-c2-support.mjs";

const SEED = { "hello.txt": "hello\n" };
const VERIFIER_PASS = JSON.stringify({ passed: true, reason: "goal verified", nextAction: null });
const VERIFIER_FAIL = JSON.stringify({ passed: false, reason: "not yet", nextAction: "continue" });

function isGoalContinuation(event: JsonObject): boolean {
  const payload = (event.payload ?? {}) as JsonObject;
  return event.type === "turn.started" && payload.inputSource === "goal-continuation";
}

function isTargetStatus(status: string) {
  return (event: JsonObject): boolean => {
    if (event.type !== "session.updated") return false;
    const payload = (event.payload ?? {}) as JsonObject;
    return (
      payload.action === "status_updated" &&
      ((payload.target as JsonObject | undefined)?.status ?? null) === status
    );
  };
}

/** 记录 goal completion verifier 的请求体（非流式、无 tools）。 */
function verifierRequest(harness: C3Harness): JsonObject | null {
  const raw = harness.rawBodies.find((body) =>
    JSON.stringify(body).includes("verification request only"),
  );
  if (raw === undefined) return null;
  const messages = Array.isArray(raw.messages) ? (raw.messages as JsonObject[]) : [];
  return {
    stream: raw.stream ?? null,
    toolCount: Array.isArray(raw.tools) ? raw.tools.length : 0,
    systemCount: Array.isArray(raw.system) ? raw.system.length : 0,
    messages: messages.map((message) => ({
      role: message.role ?? null,
      contentTypes: Array.isArray(message.content)
        ? (message.content as JsonObject[]).map((part) => part.type ?? "text")
        : typeof message.content,
      texts: (Array.isArray(message.content) ? (message.content as JsonObject[]) : [])
        .filter((part) => typeof part.text === "string")
        .map((part) => normalizeC3Deep(part.text, harness.context)),
    })),
  };
}

function verifierAux(passed: boolean) {
  return (body: { messages?: unknown[] }): { text?: string } | undefined =>
    JSON.stringify(body).includes("verification request only")
      ? { text: passed ? VERIFIER_PASS : VERIFIER_FAIL }
      : undefined;
}

async function goalSummary(harness: C3Harness, start: number): Promise<JsonObject> {
  return {
    events: projectEvents(harness.eventsSince(start)),
    eventSummary: eventSummary(harness.eventsSince(start)),
    read: projectGoalRead(await harness.readSession()),
    db: projectGoalDb(harness.readTables()),
    isolation: isolationRecord(harness),
  };
}

/** Case 1：no goal → set → 控制 turn → 续跑 turn → verifier → complete。 */
async function caseShowSetComplete(): Promise<JsonObject> {
  const harness = await startC3Harness({
    prefix: "c3-goal-complete",
    seed: SEED,
    script: [{ text: "goal answer" }],
    mockAuxiliaryResponse: verifierAux(true),
  });
  try {
    const start = harness.eventCount();
    const showEmpty = await callGoal(harness, { action: "show", inputId: "goal_show_empty" });
    const pauseEmpty = await callGoal(harness, { action: "pause", inputId: "goal_pause_empty" });
    const resumeEmpty = await callGoal(harness, { action: "resume", inputId: "goal_resume_empty" });
    const clearEmpty = await callGoal(harness, { action: "clear", inputId: "goal_clear_empty" });
    const setStart = harness.eventCount();
    const set = await callGoal(harness, {
      action: "set",
      objective: "Ship the C3 goal contract",
      inputId: "goal_set_main",
    });
    const showAfterSet = await callGoal(harness, {
      action: "show",
      inputId: "goal_show_after_set",
    });
    await harness.waitForEvent(
      setStart,
      isGoalContinuation,
      "goal continuation turn.started",
      30_000,
    );
    await harness.waitForEvent(
      setStart,
      isTargetStatus("complete"),
      "goal status_updated complete",
      30_000,
    );
    await delay(300);
    const showComplete = await callGoal(harness, { action: "show", inputId: "goal_show_complete" });
    const summary = await goalSummary(harness, start);
    return {
      isolation: summary.isolation,
      showEmpty: {
        ok: showEmpty.ok,
        response: showEmpty.ok ? showEmpty.result.response : null,
        startedTurn: showEmpty.ok ? (showEmpty.result.startedTurn ?? null) : null,
        error: showEmpty.ok ? null : showEmpty.error,
        snapshot: showEmpty.ok ? projectGoalSnapshot(showEmpty.result) : null,
      },
      noGoalActions: {
        pause: pauseEmpty.ok
          ? { response: pauseEmpty.result.response, startedTurn: pauseEmpty.result.startedTurn }
          : pauseEmpty,
        resume: resumeEmpty.ok
          ? { response: resumeEmpty.result.response, startedTurn: resumeEmpty.result.startedTurn }
          : resumeEmpty,
        clear: clearEmpty.ok
          ? { response: clearEmpty.result.response, startedTurn: clearEmpty.result.startedTurn }
          : clearEmpty,
      },
      set: {
        ok: set.ok,
        response: set.ok ? set.result.response : null,
        startedTurn: set.ok ? (set.result.startedTurn ?? null) : null,
        snapshot: set.ok ? projectGoalSnapshot(set.result) : null,
        error: set.ok ? null : set.error,
      },
      showAfterSet: showAfterSet.ok
        ? {
            response: showAfterSet.result.response,
            startedTurn: showAfterSet.result.startedTurn,
            snapshot: projectGoalSnapshot(showAfterSet.result),
          }
        : { error: showAfterSet.error },
      showComplete: {
        ok: showComplete.ok,
        response: showComplete.ok ? showComplete.result.response : null,
        startedTurn: showComplete.ok ? (showComplete.result.startedTurn ?? null) : null,
        snapshot: showComplete.ok ? projectGoalSnapshot(showComplete.result) : null,
      },
      verifierRequest: verifierRequest(harness),
      events: summary.events,
      eventSummary: summary.eventSummary,
      read: summary.read,
      db: summary.db,
    };
  } finally {
    await harness.stop();
  }
}

/** Case 2：pause（运行中）→ resume → complete → clear。 */
async function casePauseResumeClear(): Promise<JsonObject> {
  const harness = await startC3Harness({
    prefix: "c3-goal-pause",
    seed: SEED,
    script: [{ text: "slow goal answer", delayMs: 4000 }, { text: "resumed goal answer" }],
    mockAuxiliaryResponse: verifierAux(true),
  });
  try {
    const start = harness.eventCount();
    const set = await callGoal(harness, {
      action: "set",
      objective: "Pause resume clear goal",
      inputId: "goal_prc_set",
    });
    await harness.waitForEvent(start, isGoalContinuation, "goal continuation turn.started", 30_000);
    const pause = await callGoal(harness, { action: "pause", inputId: "goal_prc_pause" });
    const pauseStart = harness.eventCount();
    await harness.waitForEvent(
      pauseStart,
      (event) => {
        const payload = (event.payload ?? {}) as JsonObject;
        return event.type === "turn.completed" && payload.resultType === "cancelled";
      },
      "pause 取消当前 turn",
      30_000,
    );
    await delay(200);
    const readPaused = await harness.readSession();
    const tablesAfterPause = harness.readTables();
    const resume = await callGoal(harness, { action: "resume", inputId: "goal_prc_resume" });
    const resumeStart = harness.eventCount();
    await harness.waitForEvent(
      resumeStart,
      isTargetStatus("complete"),
      "resume 后的 verifier complete",
      30_000,
    );
    await delay(200);
    const readComplete = await harness.readSession();
    const clearStart = harness.eventCount();
    const clear = await callGoal(harness, { action: "clear", inputId: "goal_prc_clear" });
    await delay(200);
    const showAfterClear = await callGoal(harness, { action: "show", inputId: "goal_prc_show" });
    const tablesAfterClear = harness.readTables();
    return {
      isolation: isolationRecord(harness),
      set: set.ok ? { response: set.result.response, startedTurn: set.result.startedTurn } : set,
      pause: pause.ok
        ? {
            response: pause.result.response,
            startedTurn: pause.result.startedTurn,
            snapshot: projectGoalSnapshot(pause.result),
          }
        : pause,
      resume: resume.ok
        ? {
            response: resume.result.response,
            startedTurn: resume.result.startedTurn,
            snapshot: projectGoalSnapshot(resume.result),
          }
        : resume,
      clear: clear.ok
        ? {
            response: clear.result.response,
            startedTurn: clear.result.startedTurn,
            snapshot: projectGoalSnapshot(clear.result),
          }
        : clear,
      showAfterClear: showAfterClear.ok
        ? {
            response: showAfterClear.result.response,
            startedTurn: showAfterClear.result.startedTurn,
          }
        : showAfterClear,
      readPaused: projectGoalRead(readPaused),
      readComplete: projectGoalRead(readComplete),
      eventsAfterSet: projectEvents(harness.eventsSince(start)),
      eventsAfterClear: projectEvents(harness.eventsSince(clearStart)),
      eventSummary: eventSummary(harness.eventsSince(start)),
      db: {
        afterPause: projectGoalDb(tablesAfterPause),
        afterClear: projectGoalDb(tablesAfterClear),
        sessionTargetAfterPause:
          (tablesAfterPause.session_target as JsonObject | undefined)?.rows ?? null,
        sessionTargetAfterClear:
          (tablesAfterClear.session_target as JsonObject | undefined)?.rows ?? null,
      },
    };
  } finally {
    await harness.stop();
  }
}

/** Case 3：replace + expectedRevision 冲突 + 运行中管理拒绝 + 参数校验。 */
async function caseReplaceRevision(): Promise<JsonObject> {
  const harness = await startC3Harness({
    prefix: "c3-goal-replace",
    seed: SEED,
    script: [{ text: "replace goal answer", delayMs: 4000 }, { text: "second replace answer" }],
    mockAuxiliaryResponse: verifierAux(true),
  });
  try {
    const start = harness.eventCount();
    await callGoal(harness, {
      action: "set",
      objective: "Goal before replace",
      inputId: "goal_r_set",
    });
    await harness.waitForEvent(start, isGoalContinuation, "replace 前续跑开始", 30_000);
    await callGoal(harness, { action: "pause", inputId: "goal_r_pause" });
    const pauseStart = harness.eventCount();
    await harness.waitForEvent(
      pauseStart,
      (event) => {
        const payload = (event.payload ?? {}) as JsonObject;
        return event.type === "turn.completed" && payload.resultType === "cancelled";
      },
      "pause 取消",
      30_000,
    );
    const before = await harness.readSession();
    const revision = Number((before.runtime as JsonObject | undefined)?.stateRevision ?? 0);
    const beforeEventCount = harness.eventCount();
    const beforeDb = harness.readTables();
    const conflict = await callGoal(harness, {
      action: "set",
      objective: "must not apply",
      inputId: "goal_r_conflict",
      expectedRevision: Math.max(0, revision - 1),
    });
    await delay(300);
    const conflictEvents = harness.eventsSince(beforeEventCount);
    const conflictDb = harness.readTables();
    const conflictRead = await harness.readSession();
    const conflictTarget = (conflictDb.session_target as JsonObject | undefined)?.rows as
      | JsonObject[]
      | undefined;
    const replaceStart = harness.eventCount();
    const replace = await callGoal(harness, {
      action: "replace",
      objective: "Goal after replace",
      inputId: "goal_r_replace",
      expectedRevision: revision,
    });
    await harness.waitForEvent(replaceStart, isGoalContinuation, "replace 后续跑开始", 30_000);
    const whileRunning = await callGoal(harness, {
      action: "set",
      objective: "set while running",
      inputId: "goal_r_running",
    });
    await harness.waitForEvent(
      replaceStart,
      isTargetStatus("complete"),
      "replace 后 complete",
      30_000,
    );
    await delay(200);
    const after = await harness.readSession();
    const invalidAction = await callGoal(harness, { action: "bogus" });
    const tokenBudget = await callGoal(harness, {
      action: "set",
      objective: "budget",
      tokenBudget: 100,
    });
    const missingObjective = await callGoal(harness, { action: "set", inputId: "goal_r_missing" });
    const db = harness.readTables();
    return {
      isolation: isolationRecord(harness),
      conflict: {
        result: conflict.ok ? conflict.result : null,
        error: conflict.ok ? null : conflict.error,
        sideEffects: {
          newEvents: conflictEvents.length,
          stateRevisionBefore: revision,
          stateRevisionAfter: Number(
            ((conflictRead.runtime as JsonObject | undefined)?.stateRevision as
              | number
              | undefined) ?? null,
          ),
          targetObjectiveBefore: ((before.session as JsonObject | undefined)?.target as JsonObject)
            ?.objective,
          targetObjectiveAfter: ((conflictTarget?.[0] ?? null) as JsonObject | null)?.objective,
          targetRowCountBefore: (beforeDb.session_target as JsonObject | undefined)?.rowCount,
          targetRowCountAfter: (conflictDb.session_target as JsonObject | undefined)?.rowCount,
        },
      },
      replace: replace.ok
        ? {
            response: replace.result.response,
            startedTurn: replace.result.startedTurn,
            snapshot: projectGoalSnapshot(replace.result),
          }
        : replace,
      setWhileRunning: whileRunning.ok ? whileRunning.result : { error: whileRunning.error },
      invalidAction: invalidAction.ok ? invalidAction.result : { error: invalidAction.error },
      tokenBudget: tokenBudget.ok ? tokenBudget.result : { error: tokenBudget.error },
      missingObjective: missingObjective.ok
        ? {
            response: missingObjective.result.response,
            startedTurn: missingObjective.result.startedTurn,
          }
        : { error: missingObjective.error },
      readBefore: projectGoalRead(before),
      readAfter: projectGoalRead(after),
      events: projectEvents(harness.eventsSince(start)),
      eventSummary: eventSummary(harness.eventsSince(start)),
      db: projectGoalDb(db),
    };
  } finally {
    await harness.stop();
  }
}

/** Case 4：用户中断（session/stop）把 goal 置为 paused。 */
async function caseUserStop(): Promise<JsonObject> {
  const harness = await startC3Harness({
    prefix: "c3-goal-stop",
    seed: SEED,
    script: [{ text: "slow stop answer", delayMs: 5000 }],
    mockAuxiliaryResponse: verifierAux(true),
  });
  try {
    const start = harness.eventCount();
    await callGoal(harness, { action: "set", objective: "Stop goal", inputId: "goal_stop_set" });
    await harness.waitForEvent(start, isGoalContinuation, "stop 前续跑", 30_000);
    let stop: JsonObject;
    try {
      stop = (await harness.session.request("session/stop", {
        sessionId: harness.session.sessionId,
      })) as JsonObject;
    } catch (error) {
      stop = { error: String(error) };
    }
    const stopStart = harness.eventCount();
    await harness.waitForEvent(
      stopStart,
      (event) => {
        const payload = (event.payload ?? {}) as JsonObject;
        return event.type === "turn.completed" && payload.resultType === "cancelled";
      },
      "stop 后 cancelled 终态",
      30_000,
    );
    await delay(200);
    const read = await harness.readSession();
    return {
      isolation: isolationRecord(harness),
      stop,
      read: projectGoalRead(read),
      events: projectEvents(harness.eventsSince(start)),
      eventSummary: eventSummary(harness.eventsSince(start)),
      db: projectGoalDb(harness.readTables()),
    };
  } finally {
    await harness.stop();
  }
}

/** Case 5：暂停状态冷重启 → show 恢复 paused → 显式 resume 后完成。 */
async function caseColdRestart(): Promise<JsonObject> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-c3-goal-restart-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-c3-goal-restart-ws-"));
  let oldSessionId = "";
  let beforeRestart: JsonObject = {};
  try {
    const first = await startC3Harness({
      prefix: "c3-goal-restart-1",
      dataRoot,
      cwd,
      seed: SEED,
      script: [{ text: "restart slow answer", delayMs: 4000 }],
      mockAuxiliaryResponse: verifierAux(true),
    });
    try {
      oldSessionId = first.session.sessionId;
      const start = first.eventCount();
      await callGoal(first, {
        action: "set",
        objective: "Restart goal",
        inputId: "goal_restart_set",
      });
      await first.waitForEvent(start, isGoalContinuation, "restart 前续跑", 30_000);
      await callGoal(first, { action: "pause", inputId: "goal_restart_pause" });
      const pauseStart = first.eventCount();
      await first.waitForEvent(
        pauseStart,
        (event) => {
          const payload = (event.payload ?? {}) as JsonObject;
          return event.type === "turn.completed" && payload.resultType === "cancelled";
        },
        "restart 前 pause 取消",
        30_000,
      );
      await delay(200);
      beforeRestart = {
        read: projectGoalRead(await first.readSession()),
        db: projectGoalDb(first.readTables()),
      };
    } finally {
      await first.stop();
    }
    const second = await startC3Harness({
      prefix: "c3-goal-restart-2",
      dataRoot,
      cwd,
      seed: SEED,
      script: [{ text: "answer after cold restart" }],
      mockAuxiliaryResponse: verifierAux(true),
    });
    try {
      const resume = (await second.session.request("session/resume", {
        sessionId: oldSessionId,
      })) as JsonObject;
      const subscribe = (await second.session.request("session/subscribe", {
        sessionId: oldSessionId,
        deliveryKind: "desktop-continuous",
        includeSnapshot: true,
      })) as JsonObject;
      const resumeNorm = normalizeC3Deep(resume, second.context) as JsonObject;
      const subscribeNorm = normalizeC3Deep(subscribe, second.context) as JsonObject;
      const readAfterRestart = await readSessionById(second, oldSessionId);
      const showAfterRestart = await callGoal(second, {
        sessionId: oldSessionId,
        action: "show",
        inputId: "goal_restart_show",
      });
      const restartStart = second.eventCount();
      await delay(1500);
      const idleEvents = second.eventsSince(restartStart);
      const resumeGoal = await callGoal(second, {
        sessionId: oldSessionId,
        action: "resume",
        inputId: "goal_restart_resume",
      });
      await second.waitForEvent(
        restartStart,
        isTargetStatus("complete"),
        "冷恢复 resume 后 complete",
        30_000,
      );
      await delay(200);
      const readAfterResume = await readSessionById(second, oldSessionId);
      return {
        isolation: isolationRecord(second),
        oldSessionId: "<session-id>",
        beforeRestart,
        resume: {
          keys: Object.keys(resumeNorm).sort(),
          goalStats: resumeNorm.goalStats ?? null,
          snapshot: projectGoalRead(resumeNorm),
        },
        subscribe: {
          eventSeq: subscribeNorm.eventSeq ?? null,
          eventCount: Array.isArray(subscribeNorm.events) ? subscribeNorm.events.length : null,
          snapshot: projectGoalSnapshot({
            snapshot: subscribeNorm.snapshot as JsonObject,
          } as unknown as JsonObject),
        },
        readAfterRestart: projectGoalRead(readAfterRestart),
        showAfterRestart: showAfterRestart.ok
          ? {
              response: showAfterRestart.result.response,
              startedTurn: showAfterRestart.result.startedTurn,
              snapshot: projectGoalSnapshot(showAfterRestart.result),
            }
          : showAfterRestart,
        idleEventsAfterRestart: idleEvents,
        resumeGoal: resumeGoal.ok
          ? {
              response: resumeGoal.result.response,
              startedTurn: resumeGoal.result.startedTurn,
              snapshot: projectGoalSnapshot(resumeGoal.result),
            }
          : resumeGoal,
        readAfterResume: projectGoalRead(readAfterResume),
        dbAfterResume: projectGoalDb(second.readTables()),
      };
    } finally {
      await second.stop();
    }
  } finally {
    await rm(dataRoot, { recursive: true, force: true });
    await rm(cwd, { recursive: true, force: true });
  }
}

/** Case 6：运行中冷重启 → 重启后 active 自动转 paused，不重放已消费 turn。 */
async function caseActiveRestart(): Promise<JsonObject> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-c3-goal-active-restart-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-c3-goal-active-restart-ws-"));
  let oldSessionId = "";
  let beforeRestart: JsonObject = {};
  try {
    const first = await startC3Harness({
      prefix: "c3-goal-active-restart-1",
      dataRoot,
      cwd,
      seed: SEED,
      script: [{ text: "active restart slow answer", delayMs: 6000 }],
      mockAuxiliaryResponse: verifierAux(true),
    });
    try {
      oldSessionId = first.session.sessionId;
      const start = first.eventCount();
      await callGoal(first, {
        action: "set",
        objective: "Active restart goal",
        inputId: "goal_active_restart_set",
      });
      await first.waitForEvent(start, isGoalContinuation, "active restart 续跑", 30_000);
      await delay(500);
      beforeRestart = {
        read: projectGoalRead(await first.readSession()),
        db: projectGoalDb(first.readTables()),
        events: projectEvents(first.eventsSince(start)),
      };
    } finally {
      // 直接杀掉进程：模拟 turn 运行中的冷中断。
      await first.stop();
    }
    const second = await startC3Harness({
      prefix: "c3-goal-active-restart-2",
      dataRoot,
      cwd,
      seed: SEED,
      script: [{ text: "answer after active restart" }],
      mockAuxiliaryResponse: verifierAux(true),
    });
    try {
      const resume = (await second.session.request("session/resume", {
        sessionId: oldSessionId,
      })) as JsonObject;
      const resumeNorm = normalizeC3Deep(resume, second.context) as JsonObject;
      const readAfterRestart = await readSessionById(second, oldSessionId);
      const showAfterRestart = await callGoal(second, {
        sessionId: oldSessionId,
        action: "show",
      });
      const restartStart = second.eventCount();
      await delay(1500);
      return {
        isolation: isolationRecord(second),
        oldSessionId: "<session-id>",
        beforeRestart,
        resume: {
          keys: Object.keys(resumeNorm).sort(),
          goalStats: resumeNorm.goalStats ?? null,
          snapshot: projectGoalRead(resumeNorm),
        },
        readAfterRestart: projectGoalRead(readAfterRestart),
        showAfterRestart: showAfterRestart.ok
          ? {
              response: showAfterRestart.result.response,
              snapshot: projectGoalSnapshot(showAfterRestart.result),
            }
          : showAfterRestart,
        idleEventsAfterRestart: second.eventsSince(restartStart),
        dbAfterRestart: projectGoalDb(second.readTables()),
      };
    } finally {
      await second.stop();
    }
  } finally {
    await rm(dataRoot, { recursive: true, force: true });
    await rm(cwd, { recursive: true, force: true });
  }
}

/** Case 7：verifier 持续返回 passed:false → 无限续跑（有界观察 40 轮）。 */
async function caseContinuationLoop(): Promise<JsonObject> {
  const harness = await startC3Harness({
    prefix: "c3-goal-loop",
    seed: SEED,
    script: [() => ({ text: "working toward goal" })],
    mockAuxiliaryResponse: verifierAux(false),
  });
  try {
    const start = harness.eventCount();
    await callGoal(harness, { action: "set", objective: "Loop goal", inputId: "goal_loop_set" });
    const targetTurns = 40;
    const deadline = Date.now() + 60_000;
    while (true) {
      const starts = harness.eventsSince(start).filter((event) => isGoalContinuation(event)).length;
      if (starts >= targetTurns) break;
      if (Date.now() > deadline) {
        throw new Error(`loop 在 60s 内只观察到 ${starts} 个 continuation turn`);
      }
      await delay(20);
    }
    const loopTurns = harness
      .eventsSince(start)
      .filter((event) => isGoalContinuation(event)).length;
    const pauseStart = harness.eventCount();
    const pause = await callGoal(harness, { action: "pause", inputId: "goal_loop_pause" });
    // 循环可能在两次 turn 之间的瞬间被 pause：此时没有运行中的 turn 可取消，
    // 所以以「目标状态已 paused」为准，cancelled 终态是可选观察。
    await harness.waitForEvent(
      pauseStart,
      (event) =>
        isTargetStatus("paused")(event) ||
        (event.type === "turn.completed" &&
          ((event.payload ?? {}) as JsonObject).resultType === "cancelled"),
      "loop pause 生效",
      30_000,
    );
    await delay(500);
    const read = await harness.readSession();
    const events = harness.eventsSince(start);
    const verifications = events.filter((event) => {
      const payload = (event.payload ?? {}) as JsonObject;
      return event.type === "session.updated" && payload.verificationId !== undefined;
    });
    const iterations = verifications
      .map((event) => ((event.payload ?? {}) as JsonObject).goalIteration)
      .filter((value): value is number => typeof value === "number");
    const readProjection = projectGoalRead(read);
    const timeline = Array.isArray(readProjection.goalVerificationTimeline)
      ? (readProjection.goalVerificationTimeline as JsonObject[])
      : [];
    const goalVerifications = Array.isArray(readProjection.goalVerifications)
      ? (readProjection.goalVerifications as JsonObject[])
      : [];
    return {
      isolation: isolationRecord(harness),
      targetTurns,
      observedContinuationTurns: loopTurns,
      observedTerminals: events.filter(
        (event) => event.type === "turn.completed" || event.type === "turn.failed",
      ).length,
      verificationEventCount: verifications.length,
      firstIterations: iterations.slice(0, 5),
      lastIterations: iterations.slice(-5),
      sawTurnLimit: false,
      pause: pause.ok
        ? { response: pause.result.response, startedTurn: pause.result.startedTurn }
        : pause,
      read: {
        target: readProjection.target,
        sessionTarget: readProjection.sessionTarget,
        sessionStatus: readProjection.sessionStatus,
        projectionStatus: readProjection.projectionStatus,
        turnCount: readProjection.turnCount,
        eventSeq: readProjection.eventSeq,
        stateRevision: readProjection.stateRevision,
        goalVerificationCount: goalVerifications.length,
        goalVerificationsLast: goalVerifications.at(-1) ?? null,
        timelineCount: timeline.length,
        timelineFirst: timeline[0] ?? null,
        timelineLast: timeline.at(-1) ?? null,
      },
      firstEvents: projectEvents(events.slice(0, 12)),
      lastEvents: projectEvents(events.slice(-12)),
      db: projectGoalDb(harness.readTables()),
    };
  } finally {
    await harness.stop();
  }
}

const fixtures: Record<string, JsonObject> = {
  "goal-show-set-complete": {
    case: "goal-show-set-complete",
    note:
      "C3 Task 0 oracle 捕获：show(no goal) → set(objective) → 控制 turn(controlOnly) → " +
      "goal-continuation turn → target_completion_verification(passed) → target complete；" +
      "记录应答/事件/session_target/read 投影。",
    input: {
      seed: SEED,
      prompt: null,
      goal: { action: "set", objective: "Ship the C3 goal contract", inputId: "goal_set_main" },
      verifier: { passed: true, reason: "goal verified", nextAction: null },
      script: [{ text: "goal answer" }],
    },
    expect: await caseShowSetComplete(),
  },
  "goal-pause-resume-clear": {
    case: "goal-pause-resume-clear",
    note:
      "C3 Task 0 oracle 捕获：set 后续跑 turn 运行中 pause（目标立即 paused，turn 在模型返回后" +
      "以 cancelled 结束）→ resume → verifier complete → clear（target 删除，验证历史保留）。",
    input: {
      seed: SEED,
      script: [{ text: "slow goal answer", delayMs: 4000 }, { text: "resumed goal answer" }],
      goal: { action: "set", objective: "Pause resume clear goal", inputId: "goal_prc_set" },
      verifier: { passed: true, reason: "goal verified", nextAction: null },
    },
    expect: await casePauseResumeClear(),
  },
  "goal-replace-revision-conflict": {
    case: "goal-replace-revision-conflict",
    note:
      "C3 Task 0 oracle 捕获：expectedRevision 不匹配 → -32009 且零事件/零写；正确 revision 的 " +
      "replace 启动新目标；运行中 set → -32010；非法 action/未知 tokenBudget/缺 objective 的参数形状。",
    input: {
      seed: SEED,
      script: [{ text: "replace goal answer", delayMs: 4000 }, { text: "second replace answer" }],
      verifier: { passed: true, reason: "goal verified", nextAction: null },
      actions: [
        "set",
        "pause",
        "set(expectedRevision=mismatch)",
        "replace(expectedRevision=match)",
        "set(while running)",
        "invalid action",
        "set(tokenBudget=100)",
        "set(no objective)",
      ],
    },
    expect: await caseReplaceRevision(),
  },
  "goal-user-stop": {
    case: "goal-user-stop",
    note:
      "C3 Task 0 oracle 捕获：goal-continuation turn 运行中 session/stop → 目标转 paused，" +
      "turn.completed(resultType:cancelled)，之后不再自动续跑。",
    input: {
      seed: SEED,
      script: [{ text: "slow stop answer", delayMs: 5000 }],
      goal: { action: "set", objective: "Stop goal", inputId: "goal_stop_set" },
      stop: "session/stop",
    },
    expect: await caseUserStop(),
  },
  "goal-cold-restart-paused": {
    case: "goal-cold-restart-paused",
    note:
      "C3 Task 0 oracle 捕获：paused 目标冷重启后 session_target 保持 paused；session/resume " +
      "返回 goalStats，恢复后不自动续跑；显式 resume 才启动新 turn 并完成。",
    input: {
      seed: SEED,
      before: { action: "set", objective: "Restart goal", inputId: "goal_restart_set" },
      restart: "same dataRoot + cwd; session/resume(oldSessionId)",
    },
    expect: await caseColdRestart(),
  },
  "goal-cold-restart-active": {
    case: "goal-cold-restart-active",
    note:
      "C3 Task 0 oracle 捕获：goal-continuation 运行中直接杀进程后，用同一数据根 resume；" +
      "目标从 active 变为 paused，activeInput/activeRun 清空，重启后无自动 turn。",
    input: {
      seed: SEED,
      before: {
        action: "set",
        objective: "Active restart goal",
        inputId: "goal_active_restart_set",
      },
      restart: "kill oracle mid-turn; same dataRoot + cwd; session/resume(oldSessionId)",
    },
    expect: await caseActiveRestart(),
  },
  "goal-continuation-loop": {
    case: "goal-continuation-loop",
    note:
      "C3 Task 0 oracle 捕获：verifier 持续返回 passed:false 时目标持续自动续跑；" +
      "有界观察 40 个 continuation turn，未观察到轮次上限；pause 立即置 paused 并取消当前 turn。",
    input: {
      seed: SEED,
      verifier: { passed: false, reason: "not yet", nextAction: "continue" },
      script: [{ text: "working toward goal" }],
      targetTurns: 40,
    },
    expect: await caseContinuationLoop(),
  },
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
            eventTypes: (fixture.expect as JsonObject).eventSummary,
            read: (fixture.expect as JsonObject).read ?? null,
          },
        ]),
      ),
    },
    null,
    2,
  )}\n`,
);
