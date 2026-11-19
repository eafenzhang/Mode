/* oxlint-disable eslint(max-lines) -- 三个 MCP 观察组共享同一套状态轮询与投影，拆散会复制规则。 */
// C4 Task 0 观察项 1：MCP 会话面的黑盒观察。
//
// 用法：pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c4-mcp.mts
//
// 触发方式：`session/create` 带 `mcpServers` 参数（stdio 由探针启动本地最小 MCP server；
// http/sse 只录参数形状——本地 loopback 假端点，不做真实 OAuth）。
// 观察项：mcp/list 连接与状态、模型请求里的工具目录、工具调用事件、失败诊断、
// process/childProcesses 的 MCP 子进程、contextUsageBreakdown 的 mcp_tool_schemas 段。
import { createServer } from "node:http";
import { mkdir, mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { JsonObject } from "./session-lifecycle-fixtures.mjs";
import {
  isolationRecord,
  startC4Harness,
  writeC4Fixture,
  writeStdioMcpServer,
  type C4Harness,
} from "./probe-c4-support.mjs";

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

/**
 * mcp/list 状态采样。
 * 实测（见契约 §1）：不带 `mcpServers` 参数时 statuses 恒为空（workspace 配置面没有配置任何
 * server）；把与 `session/create` 相同的 mcpServers 放进**参数**里，`mode:"connect"` 才按需
 * 建连（connected + toolCount + protocolEra），`mode:"status"` 只回报 disconnected 状态。
 */
async function sampleMcpList(harness: C4Harness, mcpServers: JsonObject[]): Promise<JsonObject> {
  const status = await harness.tryRequest("mcp/list", {
    workspace: harness.workspaceRef(),
    mcpServers,
    mode: "status",
  });
  const connect = await harness.tryRequest("mcp/list", {
    workspace: harness.workspaceRef(),
    mcpServers,
    mode: "connect",
  });
  return {
    status,
    connect,
  };
}

/** 从 mock 请求体投影工具目录里 mcp__ 前缀的工具名。 */
function mcpToolNames(bodies: readonly JsonObject[]): string[] {
  for (const body of bodies) {
    if (!Array.isArray(body.tools) || (body.tools as unknown[]).length === 0) continue;
    const names = (body.tools as JsonObject[]).map((tool) => String(tool.name ?? ""));
    const mcpNames = names.filter((name) => name.startsWith("mcp__"));
    if (mcpNames.length > 0) return mcpNames;
  }
  return [];
}

/** contextUsageBreakdown 投影（只关心段标签与 mcp_tool_schemas 是否在场）。 */
function contextUsageBreakdown(events: readonly JsonObject[]): JsonObject | null {
  for (const event of events) {
    if (event.type !== "session.updated") continue;
    const payload = (event.payload ?? {}) as JsonObject;
    if (!Array.isArray(payload.contextUsageBreakdown)) continue;
    const breakdown = payload.contextUsageBreakdown as JsonObject[];
    return {
      sources: breakdown.map((segment) => String(segment.source ?? "")),
      hasMcpToolSchemas: breakdown.some((segment) => segment.source === "mcp_tool_schemas"),
    };
  }
  return null;
}

/**
 * 只保留契约定性事件：MCP 工具调用与 turn 边界。
 * `session.updated {messageCount,toolCount,iteration}` 这类用量书签事件会随流式合并时机
 * 多一条/少一条，不能作为夹具序列的一部分（实际运行已观察到 1 vs 2 条）。
 */
function stableTurnEvents(events: readonly JsonObject[]): JsonObject[] {
  return events.filter((event) => {
    if (event.type === "tool.updated") return true;
    if (
      event.type === "turn.started" ||
      event.type === "turn.completed" ||
      event.type === "turn.failed" ||
      event.type === "session.titleUpdated"
    ) {
      return true;
    }
    if (event.type === "session.updated") {
      return typeof ((event.payload ?? {}) as JsonObject).type === "string";
    }
    return false;
  });
}

/** 汇总全部 user 消息文本（MCP/skills 注入会插在真实 prompt 之后，不能只看最后一条）。 */
function allUserTexts(body: JsonObject): string[] {
  const messages = Array.isArray(body.messages) ? (body.messages as JsonObject[]) : [];
  const texts: string[] = [];
  for (const message of messages) {
    if (message.role !== "user") continue;
    if (typeof message.content === "string") texts.push(message.content);
    else if (Array.isArray(message.content)) {
      texts.push(
        (message.content as JsonObject[])
          .map((part) => (typeof part.text === "string" ? part.text : ""))
          .join("\n"),
      );
    }
  }
  return texts;
}

/** Case 1：stdio MCP server（echo / fail / slow）——工具目录、调用、失败诊断。 */
async function caseStdioHappy(): Promise<JsonObject> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-c4-mcp-happy-"));
  const serverPath = await writeStdioMcpServer(dataRoot, "happy", {
    serverName: "probe-mcp",
  });
  // 注入型合成 user 消息（skills reminder 等）会让「最后一条 user」失效，
  // 所以用触发词 + 消费标记的状态机：触发一次工具调用后，下一请求给收尾文本。
  const consumed = new Set<string>();
  const harness = await startC4Harness({
    prefix: "c4-mcp-happy",
    seed: SEED,
    dataRoot,
    script: [
      (body) => {
        const texts = allUserTexts(body);
        const has = (needle: string): boolean => texts.some((text) => text.includes(needle));
        if (has("call echo") && !consumed.has("echo")) {
          consumed.add("echo");
          return {
            toolCalls: [{ name: "mcp__probe-mcp__echo", input: { text: "roundtrip" } }],
          };
        }
        if (has("call fail") && !consumed.has("fail")) {
          consumed.add("fail");
          return { toolCalls: [{ name: "mcp__probe-mcp__fail", input: {} }] };
        }
        if (consumed.has("fail")) return { text: "fail turn done" };
        if (consumed.has("echo")) return { text: "echo turn done" };
        void lastUserText;
        return { text: "default answer" };
      },
    ],
    sessionCreateParams: {
      mcpServers: [{ name: "probe-mcp", command: process.execPath, args: [serverPath], env: [] }],
    },
  });
  try {
    const servers: JsonObject[] = [
      { name: "probe-mcp", command: process.execPath, args: [serverPath], env: [] },
    ];
    const settled = await sampleMcpList(harness, servers);

    const firstTurnStart = harness.eventCount();
    await harness.session.send("call echo please");
    // result 事件的载荷没有 toolName（C1 契约 §2.4：{toolCallId, result, duration, kind}），
    // 用 result.content 里的回显文本判定是哪次调用。
    await harness.waitForEvent(
      firstTurnStart,
      (event) => {
        const payload = (event.payload ?? {}) as JsonObject;
        const result = (payload.result ?? {}) as JsonObject;
        return (
          event.type === "tool.updated" &&
          payload.kind === "result" &&
          String(result.content ?? "").includes("echo:")
        );
      },
      "echo tool result",
      60_000,
    );
    const echoEvents = harness.eventsSince(firstTurnStart);
    // result 事件早于 turn.completed；第二个 prompt 必须等首 turn 终态，否则 -32010。
    await harness.waitForEvent(
      firstTurnStart,
      (event) => event.type === "turn.completed",
      "echo turn.completed",
      60_000,
    );

    const secondTurnStart = harness.eventCount();
    await harness.session.send("call fail now");
    await harness.waitForEvent(
      secondTurnStart,
      (event) => {
        const payload = (event.payload ?? {}) as JsonObject;
        const result = (payload.result ?? {}) as JsonObject;
        return (
          event.type === "tool.updated" &&
          payload.kind === "result" &&
          String(result.content ?? "").includes("deliberate failure")
        );
      },
      "fail tool result",
      60_000,
    );
    const failEvents = harness.eventsSince(secondTurnStart);
    const settledAfter = await sampleMcpList(harness, servers);
    // 工具目录要等 turn 跑过一次模型请求之后才录得到（rawBodies 在请求到达时填充）。
    const declaredMcpTools = mcpToolNames(harness.rawBodies);
    // MCP 子进程是 turn 首次物化工具目录时才拉起的；create 后立刻查是空表。
    const childProcesses = await harness.tryRequest("process/childProcesses", {});

    return {
      isolation: isolationRecord(harness),
      settled,
      settledAfter,
      declaredMcpTools,
      childProcesses,
      echoEvents: stableTurnEvents(echoEvents),
      failEvents: stableTurnEvents(failEvents),
      contextUsageBreakdown: contextUsageBreakdown(harness.captureState.events),
      telemetryNotifications: harness.notifications("process/mcpTelemetry").length,
    };
  } finally {
    await harness.stop();
  }
}

/** Case 2：stdio 失败诊断——立即退出的 server 与输出垃圾的 server。 */
async function caseStdioFailure(): Promise<JsonObject> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-c4-mcp-fail-"));
  const crashPath = join(dataRoot, "c4-fixtures", "crash-mcp-server.mjs");
  await mkdir(join(crashPath, ".."), { recursive: true });
  await writeFile(crashPath, "process.exit(3);\n", "utf8");
  const garbagePath = join(dataRoot, "c4-fixtures", "garbage-mcp-server.mjs");
  await writeFile(
    garbagePath,
    "console.log('this is not json');\nsetInterval(() => {}, 1000);\n",
    "utf8",
  );
  const harness = await startC4Harness({
    prefix: "c4-mcp-fail",
    seed: SEED,
    dataRoot,
    script: [{ text: "mcp failure done" }],
    sessionCreateParams: {
      mcpServers: [
        { name: "probe-crash", command: process.execPath, args: [crashPath], env: [] },
        { name: "probe-garbage", command: process.execPath, args: [garbagePath], env: [] },
      ],
    },
  });
  try {
    const servers: JsonObject[] = [
      { name: "probe-crash", command: process.execPath, args: [crashPath], env: [] },
      { name: "probe-garbage", command: process.execPath, args: [garbagePath], env: [] },
    ];
    const settled = await sampleMcpList(harness, servers);
    await harness.sendAndWait("plain prompt");
    const toolNames = mcpToolNames(harness.rawBodies);
    const childProcesses = await harness.tryRequest("process/childProcesses", {});
    return {
      isolation: isolationRecord(harness),
      settled,
      declaredMcpTools: toolNames,
      childProcesses,
      contextUsageBreakdown: contextUsageBreakdown(harness.captureState.events),
      telemetryNotifications: harness.notifications("process/mcpTelemetry").length,
    };
  } finally {
    await harness.stop();
  }
}

/** Case 3：http+oauth 参数形状——本地 loopback 假端点，只观察状态机的失败诊断。 */
async function caseHttpOauthShape(): Promise<JsonObject> {
  const requests: JsonObject[] = [];
  const httpServer = createServer((request, response) => {
    let body = "";
    request.on("data", (chunk: Buffer) => {
      body += chunk.toString();
    });
    request.on("end", () => {
      requests.push({ url: request.url ?? null, method: request.method ?? null, body });
      response.writeHead(404, { "content-type": "application/json" });
      response.end(JSON.stringify({ error: "not_found" }));
    });
  });
  await new Promise<void>((resolve) => httpServer.listen(0, "127.0.0.1", resolve));
  const address = httpServer.address();
  const port = typeof address === "object" && address !== null ? address.port : 0;
  const harness = await startC4Harness({
    prefix: "c4-mcp-http",
    seed: SEED,
    script: [{ text: "mcp http done" }],
    sessionCreateParams: {
      mcpServers: [
        {
          name: "probe-http-oauth",
          type: "http",
          url: `http://127.0.0.1:${port}/mcp`,
          headers: [],
          oauth: {
            type: "client_credentials",
            clientId: "probe-client",
            clientSecret: "probe-secret",
          },
        },
        {
          name: "probe-http-authcode",
          type: "http",
          url: `http://127.0.0.1:${port}/mcp-authcode`,
          headers: [],
          oauth: {
            type: "authorization_code",
            clientId: "probe-client",
            redirectPath: "/oauth/callback",
            scope: "mcp",
          },
        },
      ],
    },
  });
  try {
    const servers: JsonObject[] = [
      {
        name: "probe-http-oauth",
        type: "http",
        url: `http://127.0.0.1:${port}/mcp`,
        headers: [],
        oauth: {
          type: "client_credentials",
          clientId: "probe-client",
          clientSecret: "probe-secret",
        },
      },
      {
        name: "probe-http-authcode",
        type: "http",
        url: `http://127.0.0.1:${port}/mcp-authcode`,
        headers: [],
        oauth: {
          type: "authorization_code",
          clientId: "probe-client",
          redirectPath: "/oauth/callback",
          scope: "mcp",
        },
      },
    ];
    const settled = await sampleMcpList(harness, servers);
    return {
      isolation: isolationRecord(harness),
      settled,
      loopbackRequests: requests.slice(0, 6).map((request) => ({
        url: request.url,
        method: request.method,
        bodyBytes: String(request.body ?? "").length,
      })),
      childProcesses: await harness.tryRequest("process/childProcesses", {}),
    };
  } finally {
    await harness.stop();
    await new Promise<void>((resolve) => httpServer.close(() => resolve()));
  }
}

const fixtures: Record<string, JsonObject> = {
  "mcp-stdio-happy": {
    case: "mcp-stdio-happy",
    note:
      "C4 Task 0 oracle 捕获：session/create 带 stdio mcpServers 后 mcp/list(connect/status) 的" +
      "connected 形状、模型请求工具目录的 mcp__ 前缀命名、echo 调用与 fail 调用的 tool.updated " +
      "事件、process/childProcesses 的 MCP 子进程行与 contextUsageBreakdown 的 mcp_tool_schemas 段。",
    input: {
      seed: SEED,
      mcpServers: [{ name: "probe-mcp", command: "<node>", args: ["<mcp-server-path>"], env: [] }],
      prompts: ["call echo please", "call fail now"],
    },
    expect: await caseStdioHappy(),
  },
  "mcp-stdio-failure": {
    case: "mcp-stdio-failure",
    note:
      "C4 Task 0 oracle 捕获：stdio MCP server 立即退出（exit 3）与输出非 JSON 行两种失败的" +
      "mcp/list 状态机终态（failureKind/error 诊断文本）、会话是否照常创建与可用。",
    input: {
      seed: SEED,
      mcpServers: [
        { name: "probe-crash", scriptKind: "process.exit(3)" },
        { name: "probe-garbage", scriptKind: "non-json stdout" },
      ],
    },
    expect: await caseStdioFailure(),
  },
  "mcp-http-oauth-shape": {
    case: "mcp-http-oauth-shape",
    note:
      "C4 Task 0 oracle 捕获：http+oauth 参数形状（client_credentials / authorization_code 两种 " +
      "oauth 字段都被 session/create 接受）在本地 loopback 404 假端点下的状态机终态与诊断；" +
      "不做真实 OAuth。",
    input: {
      seed: SEED,
      mcpServers: [
        { name: "probe-http-oauth", oauth: "client_credentials" },
        { name: "probe-http-authcode", oauth: "authorization_code" },
      ],
      endpoint: "loopback 404",
    },
    expect: await caseHttpOauthShape(),
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
          return [
            name,
            {
              statuses: expect.settled ?? null,
              declaredMcpTools: expect.declaredMcpTools ?? null,
            },
          ];
        }),
      ),
    },
    null,
    2,
  )}\n`,
);
