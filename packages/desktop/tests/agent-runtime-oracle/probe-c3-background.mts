/* oxlint-disable eslint(max-lines) -- 空闲/忙时/与 goal 交互三类场景共享同一套通知投影与事件切片。 */
// C3 Task 0：后台 Bash 完成通知的黑盒观察（空闲、忙时、与 goal 续跑的交互）。
//
// 用法：pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c3-background.mts
//
// 关键问题（不按产品直觉写期望）：
//   1. 后台任务完成且会话空闲 → 是否自动开 turn；
//   2. 完成时另一个 turn 正在跑 → 通知被丢弃、排队还是延后；
//   3. 多个任务在忙时完成 → 每个都开 turn 还是只保留最后一个；
//   4. goal 续跑期间后台完成 → 通知 turn 与 goal verifier 的先后/计入关系。
import type { JsonObject } from "./session-lifecycle-fixtures.mjs";
import {
  callGoal,
  eventSummary,
  isolationRecord,
  projectEvents,
  projectGoalDb,
  projectGoalRead,
  projectGoalSnapshot,
  startC3Harness,
  writeC3Fixture,
  type C3Harness,
} from "./probe-c3-support.mjs";
import { delay } from "./probe-c2-support.mjs";

const SEED = { "hello.txt": "hello\n" };
const VERIFIER_PASS = JSON.stringify({ passed: true, reason: "goal verified", nextAction: null });

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

function taskIds(input: string): string[] {
  return [...input.matchAll(/<task-id>([\s\S]*?)<\/task-id>/gu)].map((match) => match[1] ?? "");
}

function taskSummaries(input: string): string[] {
  return [...input.matchAll(/<summary>([\s\S]*?)<\/summary>/gu)].map((match) => match[1] ?? "");
}

async function waitForTurnStart(
  harness: C3Harness,
  start: number,
  predicate: (payload: JsonObject) => boolean,
  what: string,
): Promise<JsonObject> {
  return harness.waitForEvent(
    start,
    (event) => event.type === "turn.started" && predicate((event.payload ?? {}) as JsonObject),
    what,
    60_000,
  );
}

/** 空闲：任务在 turn 结束后完成 → 自动开 notification turn。 */
async function caseIdle(): Promise<JsonObject> {
  const harness = await startC3Harness({
    prefix: "c3-bg-idle",
    seed: SEED,
    script: [
      (body) => {
        const text = lastUserText(body);
        if (text.includes("start background idle")) {
          return {
            toolCalls: [
              {
                name: "Bash",
                input: {
                  command: "sleep 3; printf 'idle-bg-done\\n'",
                  run_in_background: true,
                  description: "idle background marker",
                },
              },
            ],
          };
        }
        if (text.includes("<task-notification>")) return { text: "notification turn acknowledged" };
        return { text: "background started" };
      },
    ],
  });
  try {
    const start = harness.eventCount();
    await harness.session.send("start background idle");
    await harness.waitForEvent(
      start,
      (event) => event.type === "turn.completed",
      "首个 turn.completed",
      60_000,
    );
    const firstTerminalAt = harness.eventCount();
    const notificationStart = await waitForTurnStart(
      harness,
      firstTerminalAt,
      (payload) => payload.inputSource === "background_task",
      "空闲后 background_task turn.started",
    );
    const notificationWindowStart = harness.eventCount();
    await harness.waitForEvent(
      notificationWindowStart,
      (event) => event.type === "turn.completed",
      "notification turn.completed",
      60_000,
    );
    const events = harness.eventsSince(start);
    const notificationInput = String((notificationStart.payload as JsonObject)?.input ?? "");
    return {
      isolation: isolationRecord(harness),
      events: projectEvents(events),
      eventSummary: eventSummary(events),
      notificationTurn: {
        inputSource: (notificationStart.payload as JsonObject).inputSource ?? null,
        input: notificationInput,
        taskIds: taskIds(notificationInput),
        summaries: taskSummaries(notificationInput),
      },
      read: projectGoalRead(await harness.readSession()),
      db: projectGoalDb(harness.readTables()),
    };
  } finally {
    await harness.stop();
  }
}

/** 忙时：两个任务在 busy turn 中完成 → 只保留最后一个通知（晚到覆盖早到）。 */
async function caseBusyCoalesced(): Promise<JsonObject> {
  const harness = await startC3Harness({
    prefix: "c3-bg-busy",
    seed: SEED,
    script: [
      (body) => {
        const text = lastUserText(body);
        if (text.includes("start background busy")) {
          return {
            toolCalls: [
              {
                name: "Bash",
                input: {
                  command: "sleep 3; printf 'busy-one\\n'",
                  run_in_background: true,
                  description: "busy one",
                },
              },
              {
                name: "Bash",
                input: {
                  command: "sleep 3; printf 'busy-two\\n'",
                  run_in_background: true,
                  description: "busy two",
                },
              },
            ],
          };
        }
        if (text.includes("busy prompt")) return { text: "busy answer", delayMs: 9000 };
        if (text.includes("<task-notification>")) return { text: "notification handled" };
        return { text: "default answer" };
      },
    ],
  });
  try {
    const start = harness.eventCount();
    await harness.session.send("start background busy");
    await harness.waitForEvent(
      start,
      (event) => event.type === "turn.completed",
      "首个 turn.completed",
      60_000,
    );
    const busyStart = harness.eventCount();
    await harness.session.send("busy prompt");
    await harness.waitForEvent(
      busyStart,
      (event) => event.type === "turn.started",
      "busy turn.started",
      60_000,
    );
    const completedTasks = await (async () => {
      const deadline = Date.now() + 60_000;
      while (true) {
        const tasks = harness
          .eventsSince(start)
          .filter((event) => {
            const payload = (event.payload ?? {}) as JsonObject;
            return (
              event.type === "session.updated" &&
              payload.taskKind === "bash" &&
              payload.status === "completed"
            );
          })
          .map((event) => {
            const payload = (event.payload ?? {}) as JsonObject;
            return {
              description: payload.description ?? null,
              status: payload.status ?? null,
              command: payload.command ?? null,
            };
          });
        if (tasks.length >= 2) return tasks;
        if (Date.now() > deadline) throw new Error("两个后台任务没有在 60s 内完成");
        await delay(50);
      }
    })();
    await harness.waitForEvent(
      busyStart,
      (event) => event.type === "turn.completed",
      "busy turn.completed",
      60_000,
    );
    // 通知 turn 的边界从 busy turn 终态之后开始：只等这个窗口里的终态，
    // 不再用 inputId/delay 猜测是哪一轮。
    const notificationWindowStart = harness.eventCount();
    await waitForTurnStart(
      harness,
      notificationWindowStart,
      (payload) => payload.inputSource === "background_task",
      "busy 后的 background_task turn.started",
    );
    await harness.waitForEvent(
      notificationWindowStart,
      (event) => event.type === "turn.completed",
      "notification turn.completed",
      60_000,
    );
    const events = harness.eventsSince(start);
    const notificationTurns = events
      .filter((event) => event.type === "turn.started")
      .map((event) => (event.payload ?? {}) as JsonObject)
      .filter((payload) => payload.inputSource === "background_task")
      .map((payload) => {
        const input = String(payload.input ?? "");
        return { input, taskIds: taskIds(input), summaries: taskSummaries(input) };
      });
    return {
      isolation: isolationRecord(harness),
      completedTasks,
      notificationTurns,
      notificationCount: notificationTurns.length,
      deliveredSummaries: notificationTurns.flatMap((turn) => turn.summaries),
      droppedCompletedTasks: completedTasks
        .map((task) => task.description)
        .filter(
          (description) =>
            description !== null &&
            !notificationTurns.some((turn) =>
              turn.summaries.some((summary) => summary.includes(String(description))),
            ),
        ),
      events: projectEvents(events),
      eventSummary: eventSummary(events),
      read: projectGoalRead(await harness.readSession()),
      db: projectGoalDb(harness.readTables()),
    };
  } finally {
    await harness.stop();
  }
}

/** goal 续跑期间后台完成：notification turn 在 verifier 之前运行并计入 goal。 */
async function caseGoalInteraction(): Promise<JsonObject> {
  const harness = await startC3Harness({
    prefix: "c3-bg-goal",
    seed: SEED,
    script: [
      (body) => {
        const text = lastUserText(body);
        if (text.includes("Continue working toward the active session goal")) {
          if (JSON.stringify(body).includes("goal-bg-marker")) {
            return { text: "goal work continues", delayMs: 6000 };
          }
          return {
            toolCalls: [
              {
                name: "Bash",
                input: {
                  command: "sleep 2; printf 'goal-bg-marker\\n'",
                  run_in_background: true,
                  description: "goal background marker",
                },
              },
            ],
          };
        }
        if (text === "") return { text: "goal work continues", delayMs: 6000 };
        if (text.includes("<task-notification>")) return { text: "goal notification turn" };
        return { text: "goal answer" };
      },
    ],
    mockAuxiliaryResponse: (body) =>
      JSON.stringify(body).includes("verification request only")
        ? { text: VERIFIER_PASS }
        : undefined,
  });
  try {
    const start = harness.eventCount();
    const set = await callGoal(harness, {
      action: "set",
      objective: "Goal with a background task",
      inputId: "goal_bg_set",
    });
    await harness.waitForEvent(
      start,
      (event) => {
        const payload = (event.payload ?? {}) as JsonObject;
        return event.type === "turn.started" && payload.inputSource === "goal-continuation";
      },
      "goal continuation turn.started",
      60_000,
    );
    await harness.waitForEvent(
      start,
      (event) => {
        const payload = (event.payload ?? {}) as JsonObject;
        return (
          event.type === "session.updated" &&
          payload.taskKind === "bash" &&
          payload.status === "completed"
        );
      },
      "goal 后台任务 completed",
      60_000,
    );
    const notificationStart = await waitForTurnStart(
      harness,
      start,
      (payload) => payload.inputSource === "background_task",
      "goal 期间 background_task turn.started",
    );
    await harness.waitForEvent(
      start,
      (event) => {
        const payload = (event.payload ?? {}) as JsonObject;
        return (
          event.type === "session.updated" &&
          payload.action === "status_updated" &&
          (payload.target as JsonObject | undefined)?.status === "complete"
        );
      },
      "goal complete",
      60_000,
    );
    await delay(300);
    const events = harness.eventsSince(start);
    const notificationInput = String((notificationStart.payload as JsonObject)?.input ?? "");
    return {
      isolation: isolationRecord(harness),
      set: set.ok
        ? {
            response: set.result.response,
            startedTurn: set.result.startedTurn,
            snapshot: projectGoalSnapshot(set.result),
          }
        : set,
      notificationTurn: {
        inputSource: (notificationStart.payload as JsonObject).inputSource ?? null,
        input: notificationInput,
        taskIds: taskIds(notificationInput),
        summaries: taskSummaries(notificationInput),
      },
      events: projectEvents(events),
      eventSummary: eventSummary(events),
      read: projectGoalRead(await harness.readSession()),
      db: projectGoalDb(harness.readTables()),
    };
  } finally {
    await harness.stop();
  }
}

const fixtures: Record<string, JsonObject> = {
  "background-idle": {
    case: "background-idle",
    note:
      "C3 Task 0 oracle 捕获：后台 Bash（sleep 3）在主 turn 结束后完成；会话空闲时自动开 " +
      "`background_task` turn，input 是唯一一条 `<task-notification>`。",
    input: {
      seed: SEED,
      scriptKind: "user-text-keyed step function",
      background: {
        command: "sleep 3; printf 'idle-bg-done\\n'",
        description: "idle background marker",
      },
      prompt: "start background idle",
    },
    expect: await caseIdle(),
  },
  "background-busy-coalesced": {
    case: "background-busy-coalesced",
    note:
      "C3 Task 0 oracle 捕获：两个后台任务在 9s busy turn 中完成；busy 期间不立即开 turn，" +
      "busy turn 结束后启动**一个** notification turn，input 内含两条 `<task-notification>`" +
      "（按完成顺序合并；本次两条都保留）。",
    input: {
      seed: SEED,
      scriptKind: "user-text-keyed step function",
      background: [
        { command: "sleep 3; printf 'busy-one\\n'", description: "busy one" },
        { command: "sleep 3; printf 'busy-two\\n'", description: "busy two" },
      ],
      busyPrompt: "busy prompt",
      busyDelayMs: 9000,
    },
    expect: await caseBusyCoalesced(),
  },
  "background-goal-interaction": {
    case: "background-goal-interaction",
    note:
      "C3 Task 0 oracle 捕获：goal continuation turn 内启动后台 Bash；任务在续跑 turn 的第二次" +
      "模型请求期间完成。续跑 turn 结束后先运行 background_task turn，再运行 goal verifier；" +
      "notification turn 计入目标 tokensUsed。",
    input: {
      seed: SEED,
      scriptKind: "user-text-keyed step function",
      goal: { action: "set", objective: "Goal with a background task", inputId: "goal_bg_set" },
      background: {
        command: "sleep 2; printf 'goal-bg-marker\\n'",
        description: "goal background marker",
      },
      verifier: { passed: true, reason: "goal verified", nextAction: null },
    },
    expect: await caseGoalInteraction(),
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
            turns: ((fixture.expect as JsonObject).eventSummary as JsonObject)?.turnStarts ?? null,
            notification: (fixture.expect as JsonObject).notificationTurns ?? null,
            notificationTurn: (fixture.expect as JsonObject).notificationTurn ?? null,
            target: ((fixture.expect as JsonObject).read as JsonObject)?.target ?? null,
          },
        ]),
      ),
    },
    null,
    2,
  )}\n`,
);
