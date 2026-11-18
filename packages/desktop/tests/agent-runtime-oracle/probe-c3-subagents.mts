/* oxlint-disable eslint(max-lines) -- 空/前台/后台/分页四类观察共享同一套 subagents 投影与 DB 读面。 */
// C3 Task 0：`session/subagents` 只读面的黑盒观察。
//
// 用法：pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c3-subagents.mts
//
// 触发方式：模型侧通过 `Agent` 工具调用子代理（不是产品代码里的子代理运行时）。
// 观察项：空数据/首 turn 前的边界、running/ended 形状、分页游标、DB 行、
// 前台/后台子代理的 `backgroundSubagentResultConsumed` 标记。
import { Buffer } from "node:buffer";

import type { JsonObject } from "./session-lifecycle-fixtures.mjs";
import {
  eventSummary,
  isolationRecord,
  normalizeC3Deep,
  normalizeError,
  projectEvents,
  projectGoalDb,
  startC3Harness,
  writeC3Fixture,
  type C3Harness,
} from "./probe-c3-support.mjs";
import { delay } from "./probe-c2-support.mjs";

const SEED = { "hello.txt": "hello\n" };

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

async function subagents(
  harness: C3Harness,
  params: Record<string, unknown> = {},
): Promise<{ ok: true; result: JsonObject } | { ok: false; error: JsonObject }> {
  try {
    const raw = (await harness.session.request("session/subagents", {
      sessionId: harness.session.sessionId,
      ...params,
    })) as JsonObject;
    return {
      ok: true,
      result: redactCursor(normalizeC3Deep(raw, harness.context) as JsonObject),
    };
  } catch (error) {
    return { ok: false, error: normalizeError(error, harness.context) };
  }
}

function projectSubagentDb(tables: JsonObject): JsonObject {
  const sessions =
    ((tables.session as JsonObject | undefined)?.rows as JsonObject[] | undefined) ?? [];
  const links =
    ((tables.session_task_link as JsonObject | undefined)?.rows as JsonObject[] | undefined) ?? [];
  return {
    sessions: sessions.map((row) => ({
      id: row.id,
      parent_id: row.parent_id,
      task_type: row.task_type,
      title: row.title,
      slug: row.slug,
      directory: row.directory,
    })),
    sessionTaskLink: links,
    messages: projectGoalDb(tables).messageSummary,
  };
}

async function readSessionById(harness: C3Harness, sessionId: string): Promise<JsonObject> {
  let raw: JsonObject;
  try {
    raw = (await harness.session.request("session/read", { sessionId })) as JsonObject;
  } catch (error) {
    return { error: normalizeError(error, harness.context) };
  }
  const normalized = normalizeC3Deep(raw, harness.context) as JsonObject;
  const session = (normalized.session ?? {}) as JsonObject;
  const projection = (normalized.projection ?? {}) as JsonObject;
  const messages = Array.isArray(normalized.messages) ? (normalized.messages as JsonObject[]) : [];
  return {
    session: {
      sessionId: session.sessionId ?? null,
      parentSessionId: session.parentSessionId ?? null,
      sessionKind: session.sessionKind ?? null,
      title: session.title ?? null,
      status: session.status ?? null,
      mode: session.mode ?? null,
    },
    projection: {
      turnCount: projection.turnCount ?? null,
      status: projection.status ?? null,
      totalTokenCount: projection.totalTokenCount ?? null,
    },
    messageCount: messages.length,
    messageSummary: messages.map((message) => {
      const info = (message.info ?? {}) as JsonObject;
      return {
        role: info.role ?? null,
        finish: info.finish ?? null,
        semanticsKind: (info.semantics as JsonObject | undefined)?.kind ?? null,
      };
    }),
  };
}

/** 游标是不透明 base64（含 childSessionId/endedAt）；夹具只保留形状。 */
function redactCursor(result: JsonObject): JsonObject {
  const ended = result.ended as JsonObject | undefined;
  if (ended !== undefined && typeof ended.nextCursor === "string") {
    ended.nextCursor = "<cursor>";
  }
  return result;
}

function decodeCursor(cursor: string, harness: C3Harness): JsonObject | null {
  try {
    return normalizeC3Deep(
      JSON.parse(Buffer.from(cursor, "base64").toString("utf8")),
      harness.context,
    ) as JsonObject;
  } catch {
    return null;
  }
}

/** Case 1：首 turn 前的 -32004、空数据形状、endedLimit 边界。 */
async function caseEmpty(): Promise<JsonObject> {
  const harness = await startC3Harness({
    prefix: "c3-sub-empty",
    seed: SEED,
    script: [{ text: "plain answer" }],
  });
  try {
    const beforeAnyTurn = await subagents(harness, { endedLimit: 20 });
    let badSessionError: JsonObject | null = null;
    try {
      await harness.session.request("session/subagents", {
        sessionId: "sess_missing",
        endedLimit: 20,
      });
    } catch (error) {
      badSessionError = normalizeError(error, harness.context);
    }
    await harness.sendAndWait("plain prompt");
    const emptyWithLimit = await subagents(harness, { endedLimit: 20 });
    const emptyDefaultLimit = await subagents(harness);
    const zeroLimit = await subagents(harness, { endedLimit: 0 });
    const negativeLimit = await subagents(harness, { endedLimit: -1 });
    return {
      isolation: isolationRecord(harness),
      beforeAnyTurn,
      badSessionError,
      emptyWithLimit,
      emptyDefaultLimit,
      zeroLimit,
      negativeLimit,
      db: projectSubagentDb(harness.readTables()),
    };
  } finally {
    await harness.stop();
  }
}

/** Case 2：前台 Agent（general-purpose）的 running/ended 形状与 DB 行。 */
async function caseForeground(): Promise<JsonObject> {
  const harness = await startC3Harness({
    prefix: "c3-sub-foreground",
    seed: SEED,
    script: [
      (body) => {
        const text = lastUserText(body);
        if (text.includes("child task prompt")) return { text: "child result text", delayMs: 3000 };
        if (text === "") return { text: "parent after child" };
        if (text.includes("spawn foreground subagent")) {
          return {
            toolCalls: [
              {
                name: "Agent",
                input: {
                  description: "child task",
                  prompt: "child task prompt: inspect the workspace",
                  subagent_type: "general-purpose",
                },
              },
            ],
          };
        }
        return { text: "default answer" };
      },
    ],
  });
  try {
    const start = harness.eventCount();
    await harness.session.send("spawn foreground subagent");
    const runningEvent = await harness.waitForEvent(
      start,
      (event) => {
        const payload = (event.payload ?? {}) as JsonObject;
        return (
          event.type === "session.updated" &&
          payload.status === "running" &&
          payload.agentType !== undefined
        );
      },
      "subagent running session.updated",
      60_000,
    );
    const runningPayload = (runningEvent.payload ?? {}) as JsonObject;
    const runningRaw = (await harness.session.request("session/subagents", {
      sessionId: harness.session.sessionId,
      endedLimit: 20,
    })) as { running?: JsonObject[] };
    const rawChildSessionId = String(runningRaw.running?.[0]?.childSessionId ?? "");
    const running = await subagents(harness, { endedLimit: 20 });
    await harness.waitForEvent(
      start,
      (event) => {
        const payload = (event.payload ?? {}) as JsonObject;
        return (
          event.type === "session.updated" &&
          payload.status === "completed" &&
          payload.agentType !== undefined
        );
      },
      "subagent completed session.updated",
      60_000,
    );
    await harness.waitForEvent(
      start,
      (event) => event.type === "turn.completed",
      "parent turn.completed",
      60_000,
    );
    await delay(200);
    const ended = await subagents(harness, { endedLimit: 20 });
    const childRead =
      rawChildSessionId.length > 0 ? await readSessionById(harness, rawChildSessionId) : null;
    const events = harness.eventsSince(start);
    const toolResult = events
      .filter((event) => event.type === "tool.updated")
      .map((event) => (event.payload ?? {}) as JsonObject)
      .find(
        (payload) =>
          payload.kind === "result" &&
          String((payload.result as JsonObject | undefined)?.content ?? "").includes("agentId:"),
      );
    return {
      isolation: isolationRecord(harness),
      runningPayload,
      running,
      ended,
      toolResult: toolResult?.result ?? null,
      childRead,
      requests: harness.requests.map(({ body }) => ({
        toolCount: Array.isArray(body.tools) ? body.tools.length : 0,
        lastUserText: String(normalizeC3Deep(lastUserText(body), harness.context)).slice(-160),
      })),
      events: projectEvents(events),
      eventSummary: eventSummary(events),
      db: projectSubagentDb(harness.readTables()),
    };
  } finally {
    await harness.stop();
  }
}

/** Case 3：后台 Agent → notification turn 的 `backgroundSubagentResultConsumed`。 */
async function caseBackground(): Promise<JsonObject> {
  const harness = await startC3Harness({
    prefix: "c3-sub-background",
    seed: SEED,
    script: [
      (body) => {
        const text = lastUserText(body);
        if (text.includes("background child prompt")) {
          return { text: "background child result", delayMs: 2000 };
        }
        if (text.includes("<task-notification>")) return { text: "notification turn" };
        if (text === "") return { text: "parent after background child" };
        if (text.includes("spawn background subagent")) {
          return {
            toolCalls: [
              {
                name: "Agent",
                input: {
                  description: "background child",
                  prompt: "background child prompt: inspect in background",
                  subagent_type: "general-purpose",
                  run_in_background: true,
                },
              },
            ],
          };
        }
        return { text: "default answer" };
      },
    ],
  });
  try {
    const start = harness.eventCount();
    await harness.session.send("spawn background subagent");
    await harness.waitForEvent(
      start,
      (event) => {
        const payload = (event.payload ?? {}) as JsonObject;
        return (
          event.type === "session.updated" &&
          payload.status === "running" &&
          payload.agentType !== undefined
        );
      },
      "background subagent running",
      60_000,
    );
    const mid = await subagents(harness, { endedLimit: 20 });
    const notificationStart = await harness.waitForEvent(
      start,
      (event) => {
        const payload = (event.payload ?? {}) as JsonObject;
        return event.type === "turn.started" && payload.inputSource === "background_task";
      },
      "background subagent notification turn",
      60_000,
    );
    const notificationTerminal = await harness.waitForEvent(
      start,
      (event) =>
        event.type === "turn.completed" &&
        (event.payload as JsonObject | undefined)?.backgroundSubagentResultConsumed === true,
      "backgroundSubagentResultConsumed turn.completed",
      60_000,
    );
    await delay(200);
    const after = await subagents(harness, { endedLimit: 20 });
    const events = harness.eventsSince(start);
    return {
      isolation: isolationRecord(harness),
      mid,
      after,
      notificationTurn: {
        inputSource: (notificationStart.payload as JsonObject).inputSource ?? null,
        input: String((notificationStart.payload as JsonObject).input ?? ""),
      },
      notificationTerminal: notificationTerminal.payload,
      events: projectEvents(events),
      eventSummary: eventSummary(events),
      db: projectSubagentDb(harness.readTables()),
    };
  } finally {
    await harness.stop();
  }
}

/** Case 4：两个子代理 + endedLimit/endedCursor 分页。 */
async function casePagination(): Promise<JsonObject> {
  const harness = await startC3Harness({
    prefix: "c3-sub-pagination",
    seed: SEED,
    script: [
      (body) => {
        const text = lastUserText(body);
        if (text.includes("child one prompt")) return { text: "child one result", delayMs: 2000 };
        if (text.includes("child two prompt")) return { text: "child two result", delayMs: 2500 };
        if (text === "") return { text: "parent after two children" };
        if (text.includes("spawn two subagents")) {
          return {
            toolCalls: [
              {
                name: "Agent",
                input: {
                  description: "child one",
                  prompt: "child one prompt",
                  subagent_type: "general-purpose",
                },
              },
              {
                name: "Agent",
                input: {
                  description: "child two",
                  prompt: "child two prompt",
                  subagent_type: "Explore",
                },
              },
            ],
          };
        }
        return { text: "default answer" };
      },
    ],
  });
  try {
    const start = harness.eventCount();
    await harness.session.send("spawn two subagents");
    await harness.waitForEvent(
      start,
      (event) => event.type === "turn.completed",
      "pagination parent turn.completed",
      60_000,
    );
    await delay(200);
    const rawFirst = (await harness.session.request("session/subagents", {
      sessionId: harness.session.sessionId,
      endedLimit: 1,
    })) as JsonObject;
    const first = redactCursor(normalizeC3Deep(rawFirst, harness.context) as JsonObject);
    const rawCursor = String(
      ((rawFirst.ended as JsonObject | undefined)?.nextCursor as string | undefined) ?? "",
    );
    const second = rawCursor
      ? await (async () => {
          try {
            const raw = (await harness.session.request("session/subagents", {
              sessionId: harness.session.sessionId,
              endedLimit: 1,
              endedCursor: rawCursor,
            })) as JsonObject;
            return { ok: true, result: normalizeC3Deep(raw, harness.context) as JsonObject };
          } catch (error) {
            return { ok: false, error: normalizeError(error, harness.context) };
          }
        })()
      : null;
    const zeroLimit = await subagents(harness, { endedLimit: 0 });
    const events = harness.eventsSince(start);
    const isAgentLifecycle = (event: JsonObject): boolean => {
      const payload = (event.payload ?? {}) as JsonObject;
      return (
        event.type === "session.updated" &&
        payload.agentType !== undefined &&
        payload.status !== undefined
      );
    };
    const lifecycle = events
      .filter((event) => isAgentLifecycle(event))
      .sort((left, right) => {
        const leftPayload = (left.payload ?? {}) as JsonObject;
        const rightPayload = (right.payload ?? {}) as JsonObject;
        return String(leftPayload.description ?? leftPayload.prompt ?? "").localeCompare(
          String(rightPayload.description ?? rightPayload.prompt ?? ""),
        );
      });
    const db = projectSubagentDb(harness.readTables());
    const sessions = (db.sessions as JsonObject[])
      .slice()
      .sort((left, right) => String(left.title).localeCompare(String(right.title)));
    const messageRows =
      ((harness.readTables().message as JsonObject | undefined)?.rows as
        | JsonObject[]
        | undefined) ?? [];
    return {
      isolation: isolationRecord(harness),
      firstPage: first,
      cursor: {
        present: rawCursor.length > 0,
        decoded: rawCursor.length > 0 ? decodeCursor(rawCursor, harness) : null,
      },
      secondPage: second,
      zeroLimit,
      // 并行 Agent 的 running/completed 事件到达顺序不稳定：按 description 排序记录。
      agentLifecycle: projectEvents(lifecycle),
      events: projectEvents(events.filter((event) => !isAgentLifecycle(event))),
      eventSummary: eventSummary(events),
      db: {
        sessions,
        sessionTaskLink: db.sessionTaskLink,
        messageCount: messageRows.length,
      },
    };
  } finally {
    await harness.stop();
  }
}

const fixtures: Record<string, JsonObject> = {
  "subagents-empty": {
    case: "subagents-empty",
    note:
      "C3 Task 0 oracle 捕获：首 turn 前 `session/subagents` 返回 -32004（Session not found）；" +
      "有 turn 后返回空列表 `{revision,childSessionIds:[],running:[],ended:{total:0,items:[]}}`；" +
      "endedLimit 0/-1 是 -32602（>0）。",
    input: {
      seed: SEED,
      prompt: "plain prompt",
      calls: ["before-turn", "endedLimit:20", "default", "endedLimit:0", "endedLimit:-1"],
    },
    expect: await caseEmpty(),
  },
  "subagents-foreground": {
    case: "subagents-foreground",
    note:
      "C3 Task 0 oracle 捕获：前台 Agent(general-purpose) 的 running/ended 形状、session.updated " +
      "running/completed 生命周期、工具结果文案、child session 读面与 session_task_link 行。",
    input: {
      seed: SEED,
      scriptKind: "user-text-keyed step function",
      agent: { subagent_type: "general-purpose", description: "child task" },
      foreground: true,
    },
    expect: await caseForeground(),
  },
  "subagents-background": {
    case: "subagents-background",
    note:
      "C3 Task 0 oracle 捕获：后台 Agent 完成后由 `background_task` notification turn 消费结果，" +
      "turn.completed 带 `backgroundSubagentResultConsumed:true`；subagents ended 项 summary 是子代理结果。",
    input: {
      seed: SEED,
      scriptKind: "user-text-keyed step function",
      agent: {
        subagent_type: "general-purpose",
        description: "background child",
        run_in_background: true,
      },
    },
    expect: await caseBackground(),
  },
  "subagents-pagination": {
    case: "subagents-pagination",
    note:
      "C3 Task 0 oracle 捕获：两个已结束子代理按 endedAt 倒序分页；endedLimit:1 返回最新一个与 " +
      "不透明 base64 nextCursor，第二页返回剩余项；childSessionIds 保持创建顺序。",
    input: {
      seed: SEED,
      scriptKind: "user-text-keyed step function",
      agents: [
        { subagent_type: "general-purpose", description: "child one" },
        { subagent_type: "Explore", description: "child two" },
      ],
      pageSize: 1,
    },
    expect: await casePagination(),
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
            running: ((fixture.expect as JsonObject).running as JsonObject)?.result ?? null,
            ended: ((fixture.expect as JsonObject).ended as JsonObject)?.result ?? null,
            firstPage: ((fixture.expect as JsonObject).firstPage as JsonObject)?.ended ?? null,
            markers: ((fixture.expect as JsonObject).eventSummary as JsonObject)?.terminals ?? null,
          },
        ]),
      ),
    },
    null,
    2,
  )}\n`,
);
