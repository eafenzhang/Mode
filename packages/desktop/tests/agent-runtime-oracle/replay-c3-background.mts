// C3 Task 5：background 夹具回放的 case drivers（`replay-c3.mts` 的比较半边；
// 比较面与登记见 replay-c3.mts 文件头；retry/subagents drivers 在 replay-c3-mixed.mts）。
//
// 剧本重建：夹具 `input` 的 user-text-keyed step function 在 probe 里是函数、JSON 只存
// 参数——按同一语义重建（与 probe-c3-background 的步骤逐字对应）。
import type { JsonObject } from "./session-lifecycle-fixtures.mjs";
import { normalizeC3Deep, projectGoalDb, projectGoalRead } from "./probe-c3-support.mjs";
import { delay } from "./probe-c2-support.mjs";
import {
  diffC3Value,
  diffEventStream,
  diffGoalDb,
  PLAIN_EVENT_SCOPE,
  startC3Replay,
  stopC3Replay,
  type C3Diff,
  type C3Fixture,
  type C3ReplayResult,
  type C3Run,
} from "./replay-c3.mjs";
import {
  backgroundScript,
  foldRead,
  notificationTurnsOf,
  taskSummaries,
} from "./replay-c3-mixed.mjs";

// background 共用驱动面（idle / busy-coalesced）
// ---------------------------------------------------------------------------

function taskIds(input: string): string[] {
  return [...input.matchAll(/<task-id>([\s\S]*?)<\/task-id>/gu)].map((match) => match[1] ?? "");
}

function taskSummaries(input: string): string[] {
  return [...input.matchAll(/<summary>([\s\S]*?)<\/summary>/gu)].map((match) => match[1] ?? "");
}

function notificationTurnsOf(run: C3Run, events: { type: string; payload: unknown }[]): JsonObject[] {
  return events
    .filter((event) => event.type === "turn.started")
    .map((event) => (event.payload ?? {}) as JsonObject)
    .filter((payload) => payload.inputSource === "background_task")
    .map((payload) => {
      // 实际侧 input 先过与录制同一份规范化（data-root/会话 id/exec id 折叠）。
      const input = String(normalizeC3Deep(payload.input ?? "", run.context, run.aliases));
      return { input, taskIds: taskIds(input), summaries: taskSummaries(input) };
    });
}

async function replayBackgroundIdle(fixture: C3Fixture): Promise<C3ReplayResult> {
  const diffs: C3Diff[] = [];
  const notes: string[] = [];
  const background = fixture.input.background as BackgroundSpec;
  const run = await startC3Replay({
    fixture,
    mockScript: backgroundScript("start background idle", [background], undefined, 0, "background started"),
  });
  try {
    const expect = fixture.expect;
    const start = run.frames().length;
    await run.session.send("start background idle");
    await run.waitForEvent(start, (event) => event.type === "turn.completed", "首个 turn.completed");
    const notification = await run.waitForEvent(
      start,
      (event) => {
        const payload = (event.payload ?? {}) as JsonObject;
        return event.type === "turn.started" && payload.inputSource === "background_task";
      },
      "空闲后 background_task turn.started",
    );
    await run.waitForEvent(start, (event) => event.type === "turn.completed", "notification turn.completed");
    await delay(300);
    const input = String(
      normalizeC3Deep((notification.payload as JsonObject).input ?? "", run.context, run.aliases),
    );
    diffs.push(...diffC3Value("response", "notificationTurn", {
      inputSource: (notification.payload as JsonObject).inputSource ?? null,
      input,
      taskIds: taskIds(input),
      summaries: taskSummaries(input),
    }, expect.notificationTurn as JsonObject));
    diffs.push(...diffEventStream(run.normalizedActualEvents(start, PLAIN_EVENT_SCOPE), run.normalizedFixtureEvents((expect.events as JsonObject[]) ?? [], PLAIN_EVENT_SCOPE)));
    diffs.push(...diffC3Value("response", "read", foldRead(projectGoalRead(await run.readSessionNormalized())), foldRead(expect.read as JsonObject)));
    diffs.push(...diffGoalDb("db", projectGoalDb(run.readTables()), expect.db as JsonObject, { dropModelUsage: true }));
    notes.push("任务状态 session.updated（taskId）与 per-turn model_usage 不比（§3.5/§2.7 登记）");
    if (run.session.invalidLines.length > 0) {
      throw new Error(`replay-c3：stdout 解析失败行：${run.session.invalidLines[0]}`);
    }
    return { case: fixture.case, diffs, notes, stderr: run.session.stderr };
  } finally {
    await stopC3Replay(run);
  }
}

/** 读面折叠（两侧同一实现）：eventSeq/stateRevision/turnCount 绝对值登记差异不比。 */
function foldRead(read: unknown): JsonObject {
  const folded = { ...((read ?? {}) as JsonObject) };
  delete folded.eventSeq;
  delete folded.stateRevision;
  delete folded.turnCount;
  return folded;
}

async function replayBackgroundBusy(fixture: C3Fixture): Promise<C3ReplayResult> {
  const diffs: C3Diff[] = [];
  const notes: string[] = [];
  const backgrounds = fixture.input.background as BackgroundSpec[];
  const busyDelayMs = Number(fixture.input.busyDelayMs ?? 9000);
  // 两个任务的完成顺序决定合并通知的顺序（§3.2 按完成顺序合并）；命令文本在
  // tool_input_delta/tool_call.input 逐字比较面里，不能用改 sleep 制造确定序——
  // 完成序由 OS 调度决定，合并顺序按「时序相关结果」处理（C2 same-tick 同款）：
  // 集合相等 + 注记观察顺序。
  const run = await startC3Replay({
    fixture,
    mockScript: backgroundScript("start background busy", backgrounds, "busy prompt", busyDelayMs, "default answer"),
  });
  try {
    const expect = fixture.expect;
    const start = run.frames().length;
    await run.session.send("start background busy");
    await run.waitForEvent(start, (event) => event.type === "turn.completed", "首个 turn.completed");
    // 与 oracle 探针同驾驶序：busy prompt 在 turn0 完成后发送；其 9s 模型延迟覆盖
    // sleep 3——两个后台任务必然在 busy turn 的模型流式等待窗口内完成（§3.2 合并）。
    await run.session.send("busy prompt");
    const busyStart = run.frames().length;
    await run.waitForEvent(busyStart, (event) => event.type === "turn.completed", "busy turn.completed", 120_000);
    // 通知 turn 的等待从 busyStart 起搜（涵盖「通知 turn 与 busy 终态帧的到达序」
    // 的极端时序）；通知 turn 的终态单独等（落库沉降），超时不失败——比较面仍以
    // 事件流本身为准。
    await run.waitForEvent(
      busyStart,
      (event) => {
        const payload = (event.payload ?? {}) as JsonObject;
        return event.type === "turn.started" && payload.inputSource === "background_task";
      },
      "busy 后的 background_task turn.started",
    );
    await run
      .waitForEvent(run.frames().length, (event) => event.type === "turn.completed", "notification turn.completed", 30_000)
      .catch(() => undefined);
    await delay(300);
    const events = run.frames().slice(start);
    const turns = notificationTurnsOf(run, events);
    diffs.push(...diffC3Value("rows", "notificationCount", turns.length, expect.notificationCount ?? null));
    diffs.push(...diffC3Value("rows", "deliveredSummaries.sorted", turns.flatMap((turn) => turn.summaries).sort(), ((expect.deliveredSummaries ?? []) as string[]).slice().sort()));
    notes.push(
      `合并通知顺序按完成顺序（§3.2）：观察序 [${turns.flatMap((turn) => turn.summaries).map((summary) => (summary.includes("busy one") ? "one" : "two")).join(", ")}]；` +
        "任务完成序由 OS 调度决定，夹具记录的顺序与回放序都是登记结果（时序相关）",
    );
    const descriptions = backgrounds.map((background) => background.description);
    diffs.push(...diffC3Value("rows", "droppedCompletedTasks", descriptions.filter((description) => !turns.some((turn) => (turn.summaries as string[]).some((summary) => summary.includes(description)))), expect.droppedCompletedTasks ?? null));
    diffs.push(...diffEventStream(run.normalizedActualEvents(start, PLAIN_EVENT_SCOPE), run.normalizedFixtureEvents((expect.events as JsonObject[]) ?? [], PLAIN_EVENT_SCOPE)));
    diffs.push(...diffC3Value("response", "read", foldRead(projectGoalRead(await run.readSessionNormalized())), foldRead(expect.read as JsonObject)));
    diffs.push(...diffGoalDb("db", projectGoalDb(run.readTables()), expect.db as JsonObject, { dropModelUsage: true }));
    notes.push("completedTasks 不比（任务状态事件未产出，§3.5）；busy 合并窗口 = 模型流式等待（§3.2）");
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

export function registerC3BackgroundCases(registry: Record<string, (fixture: C3Fixture) => Promise<C3ReplayResult>>): void {
  registry["background-idle"] = replayBackgroundIdle;
  registry["background-busy-coalesced"] = replayBackgroundBusy;
}
