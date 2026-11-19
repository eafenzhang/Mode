/* oxlint-disable eslint(max-lines) -- Agent 工具定义、child 请求投影与 child DB 行共用同一套观察口径。 */
// C4 Task 0 观察项 8：子代理执行面（`Agent` 工具 → child turn）的黑盒观察。
//
// 用法：pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c4-subagent-exec.mts
//
// C3 已录过 `session/subagents` 只读面与生命周期事件（agent-c3-contract.md §4）；
// C4 T5 要落「child turn 最小闭环」，还缺三块形状，本探针补齐：
//   1. 父请求里 `Agent` 工具的定义（name/description/input_schema 逐字）；
//   2. child 模型请求与父请求的差异（system 段、messages、toolNames——C3 只录了 toolCount）；
//   3. child 会话在 DB 里的行（session/message/part），以及工具结果与 usage 文本。
import { DatabaseSync } from "node:sqlite";

import type { JsonObject } from "./session-lifecycle-fixtures.mjs";
import { eventSummary, isolationRecord, normalizeC3Deep } from "./probe-c3-support.mjs";
import { startC4Harness, writeC4Fixture, type C4Harness } from "./probe-c4-support.mjs";
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

/** 从原始请求体取某个工具定义（Anthropic 请求里 schema 键是 `input_schema`）。 */
function findTool(body: JsonObject | undefined, name: string): JsonObject | null {
  const tools = Array.isArray(body?.tools) ? (body?.tools as JsonObject[]) : [];
  const tool = tools.find((candidate) => candidate.name === name);
  if (tool === undefined) return null;
  return {
    name: tool.name ?? null,
    description: tool.description ?? null,
    input_schema: tool.input_schema ?? tool.inputSchema ?? null,
  };
}

/** 请求体投影：直接复用 C2 harness 的 `bodies`（system 段 chars/head、messages 投影、toolNames）。 */
function projectBodies(harness: C4Harness): JsonObject[] {
  return harness.bodies.map((entry, index) => {
    const body = (entry.body ?? {}) as JsonObject;
    const messages = Array.isArray(body.messages) ? (body.messages as JsonObject[]) : [];
    const lastUser = [...messages].reverse().find((message) => message.role === "user");
    const userHeads = Array.isArray(lastUser?.textHeads) ? (lastUser?.textHeads as string[]) : [];
    return {
      index,
      path: entry.path ?? null,
      model: body.model ?? null,
      maxTokens: body.max_tokens ?? null,
      stream: body.stream ?? null,
      thinking: body.thinking ?? null,
      keys: body.keys ?? null,
      toolCount: body.toolCount ?? null,
      toolNames: body.toolNames ?? null,
      system: body.system ?? null,
      messages: messages.map((message) => ({
        role: message.role ?? null,
        contentTypes: message.contentTypes ?? null,
        textChars: message.textChars ?? null,
        textHeads: message.textHeads ?? null,
      })),
      isChild: userHeads.some((head) => head.includes("child execute prompt")),
      lastUserTail: typeof lastUser?.lastTextTail === "string" ? lastUser.lastTextTail : null,
    };
  });
}

/**
 * child 会话行：用原始 childSessionId 直查 message/part（`readTables` 的规范化会把
 * 所有 id 折叠成 `<id>`，无法按会话区分）。返回的文本仍走同一套规范化。
 */
function readChildRows(harness: C4Harness, childSessionId: string): JsonObject {
  const db = new DatabaseSync(harness.dbPath, { readOnly: true });
  try {
    const sessionColumns = db.prepare("pragma table_info(session)").all() as JsonObject[];
    const sessions = db
      .prepare("select * from session where id = ?")
      .all(childSessionId) as JsonObject[];
    const messageColumns = db.prepare("pragma table_info(message)").all() as JsonObject[];
    const hasSessionId = messageColumns.some((column) => column.name === "session_id");
    const messages = hasSessionId
      ? (db
          .prepare("select * from message where session_id = ? order by rowid")
          .all(childSessionId) as JsonObject[])
      : [];
    const parts = db
      .prepare(
        "select p.* from part p join message m on m.id = p.message_id where m.session_id = ? order by p.rowid",
      )
      .all(childSessionId) as JsonObject[];
    const partsByMessage = new Map<string, string[]>();
    for (const part of parts) {
      const data = typeof part.data === "string" ? JSON.parse(part.data) : part.data;
      const list = partsByMessage.get(String(part.message_id)) ?? [];
      list.push(String((data as JsonObject | undefined)?.type ?? "unknown"));
      partsByMessage.set(String(part.message_id), list);
    }
    return {
      sessionColumns: sessionColumns.map((column) => column.name),
      sessions: sessions.map((row) => {
        const normalized = normalizeC3Deep(row, harness.context, harness.aliases) as JsonObject;
        return {
          id: normalized.id,
          parent_id: normalized.parent_id,
          task_type: normalized.task_type,
          title: normalized.title,
          slug: normalized.slug,
          directory: normalized.directory,
        };
      }),
      messages: messages.map((row) => {
        const data = typeof row.data === "string" ? JSON.parse(row.data) : row.data;
        const info = (data ?? {}) as JsonObject;
        const semantics = (info.semantics ?? {}) as JsonObject;
        return {
          sequence: row.sequence,
          role: info.role ?? null,
          finish: info.finish ?? null,
          semanticsKind: semantics.kind ?? null,
          source: semantics.source ?? null,
          partTypes: partsByMessage.get(String(row.id)) ?? [],
        };
      }),
      partCount: parts.length,
    };
  } finally {
    db.close();
  }
}

/** 工具结果投影（`tool.updated kind:"result"` 且正文带 agentId 的那条）。 */
function projectAgentToolResult(events: readonly JsonObject[]): JsonObject | null {
  for (const event of events) {
    if (event.type !== "tool.updated") continue;
    const payload = (event.payload ?? {}) as JsonObject;
    if (payload.kind !== "result") continue;
    const result = (payload.result ?? {}) as JsonObject;
    const content = typeof result.content === "string" ? result.content : "";
    if (!content.includes("agentId:")) continue;
    return {
      toolCallId: payload.toolCallId ?? null,
      input: payload.input ?? null,
      content,
      truncated: result.truncated ?? null,
      budgetStrategy: result.budgetStrategy ?? null,
      originalBytes: result.originalBytes ?? null,
      returnedBytes: result.returnedBytes ?? null,
      metadata: result.metadata ?? null,
    };
  }
  return null;
}

/** Case 1：前台 `Agent`（general-purpose）的 child turn 请求/DB/结果形状。 */
async function caseForegroundExecution(): Promise<JsonObject> {
  const harness = await startC4Harness({
    prefix: "c4-subagent-exec",
    seed: SEED,
    script: [
      (body) => {
        const text = lastUserText(body);
        if (text.includes("child execute prompt"))
          return { text: "child final text", delayMs: 300 };
        if (text === "") return { text: "parent after child" };
        if (text.includes("spawn child execute")) {
          return {
            toolCalls: [
              {
                name: "Agent",
                input: {
                  description: "exec child",
                  prompt: "child execute prompt: inspect only",
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
    await harness.session.send("spawn child execute");
    await harness.waitForEvent(
      start,
      (event) => event.type === "turn.completed",
      "parent turn.completed",
      120_000,
    );
    await delay(200);
    // 原始 childSessionId 要在规范化之前取（别名状态只用于夹具投影）。
    const rawSubagents = (await harness.session.request("session/subagents", {
      sessionId: harness.session.sessionId,
      endedLimit: 20,
    })) as JsonObject;
    const rawChildSessionId = String(rawSubagents.childSessionIds?.[0] ?? "");
    const events = harness.eventsSince(start);
    const parentBody = harness.rawBodies.find((body) => findTool(body, "Agent") !== null);
    const ended = await harness.tryRequest("session/subagents", {
      sessionId: harness.session.sessionId,
      endedLimit: 20,
    });
    return {
      isolation: isolationRecord(harness),
      agentTool: findTool(parentBody, "Agent"),
      requests: projectBodies(harness),
      toolResult: projectAgentToolResult(events),
      childRows: rawChildSessionId.length > 0 ? readChildRows(harness, rawChildSessionId) : null,
      ended,
      eventSummary: eventSummary(events),
      eventNames: events.map((event) => event.type),
      dbSessions: ((harness.readTables().session as JsonObject | undefined)?.rows ??
        []) as JsonObject[],
    };
  } finally {
    await harness.stop();
  }
}

const fixtures: Record<string, JsonObject> = {
  "subagent-execute-foreground": {
    case: "subagent-execute-foreground",
    note:
      "C4 Task 0 oracle 捕获：父请求里的 `Agent` 工具定义（name/description/input_schema 逐字）、" +
      "child 请求与父请求的 system/messages/toolNames 差异、child 会话 session/message/part 行、" +
      "Agent 工具结果（agentId/usage）与 `session.updated` 生命周期事件摘要。",
    input: {
      seed: SEED,
      scriptKind: "user-text-keyed step function",
      agent: { subagent_type: "general-purpose", description: "exec child" },
      foreground: true,
    },
    expect: await caseForegroundExecution(),
  },
};

const targets: string[] = [];
for (const [name, fixture] of Object.entries(fixtures)) {
  targets.push(await writeC4Fixture(name, fixture));
}
process.stdout.write(
  `${JSON.stringify(
    {
      fixtures: targets,
      summaries: Object.fromEntries(
        Object.entries(fixtures).map(([name, fixture]) => {
          const expect = fixture.expect as JsonObject;
          const requests = Array.isArray(expect.requests) ? (expect.requests as JsonObject[]) : [];
          return [
            name,
            {
              agentToolName: (expect.agentTool as JsonObject | null)?.name ?? null,
              requestCount: requests.length,
              requestTools: requests.map((request) => ({
                index: request.index,
                isChild: request.isChild,
                toolCount: request.toolCount,
              })),
              toolResultHead: String((expect.toolResult as JsonObject | null)?.content ?? "").slice(
                0,
                120,
              ),
            },
          ];
        }),
      ),
    },
    null,
    2,
  )}\n`,
);
