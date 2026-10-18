// M3 插件宿主 Task 0.3/5：fixture 插件暴露一个 MCP 工具，观察发现与调用。
//
// 用法：
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-plugin-mcp.mts
//
// 自包含：本地 marketplace → 安装插件（含 .mcp.json + 最小 MCP stdio server）→
// 同一数据根启动 oracle → 调用 `mcp__fixture__echo`。只观察行为，不读实现体。
import { execFileSync } from "node:child_process";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { startOracleRuntime } from "./driver.mjs";

const REPO_ROOT = fileURLToPath(new URL("../../../../", import.meta.url));
const TSX_CLI = join(REPO_ROOT, "node_modules", "tsx", "dist", "cli.mjs");
const CLI_ENTRY = join(REPO_ROOT, "apps", "mode-cli", "packages", "cli", "src", "main.ts");

const dataRoot = await mkdtemp(join(tmpdir(), "mode-probe-mcp-"));
const cwd = await mkdtemp(join(tmpdir(), "mode-probe-mcp-ws-"));
const marketplaceRoot = await mkdtemp(join(tmpdir(), "mode-probe-mcp-market-"));
const pluginRoot = join(marketplaceRoot, "plugins", "fixture");
const serverPath = join(pluginRoot, "mcp-server.mjs");
const env = {
  ...process.env,
  HOME: dataRoot,
  USERPROFILE: dataRoot,
  MODE_HOME: dataRoot,
  MODE_DATA_BASE_DIR: dataRoot,
};

const MCP_SERVER = [
  'import { createInterface } from "node:readline";',
  "const rl = createInterface({ input: process.stdin });",
  "const send = (value) => process.stdout.write(JSON.stringify(value) + '\\n');",
  "rl.on('line', (line) => {",
  "  let message;",
  "  try { message = JSON.parse(line); } catch { return; }",
  "  if (message.id === undefined) return;",
  "  if (message.method === 'initialize') {",
  "    send({ jsonrpc: '2.0', id: message.id, result: { protocolVersion: '2024-11-05', capabilities: { tools: {} }, serverInfo: { name: 'fixture', version: '1.0.0' } } });",
  "    return;",
  "  }",
  "  if (message.method === 'tools/list') {",
  "    send({ jsonrpc: '2.0', id: message.id, result: { tools: [{ name: 'echo', description: 'Echo text', inputSchema: { type: 'object', properties: { text: { type: 'string' } }, required: ['text'], additionalProperties: false } }] } });",
  "    return;",
  "  }",
  "  if (message.method === 'tools/call') {",
  "    const text = (message.params && message.params.arguments && message.params.arguments.text) || '';",
  "    send({ jsonrpc: '2.0', id: message.id, result: { content: [{ type: 'text', text: String(text) }] } });",
  "    return;",
  "  }",
  "  if (message.method === 'ping') { send({ jsonrpc: '2.0', id: message.id, result: {} }); return; }",
  "  send({ jsonrpc: '2.0', id: message.id, error: { code: -32601, message: 'method not found: ' + message.method } });",
  "});",
  "",
].join("\n");

await mkdir(join(pluginRoot, ".tutto-plugin"), { recursive: true });
await writeFile(
  join(pluginRoot, ".tutto-plugin", "plugin.json"),
  JSON.stringify({ name: "fixture", version: "1.0.0", description: "Fixture MCP plugin" }, null, 2),
  "utf8",
);
await writeFile(
  join(pluginRoot, ".mcp.json"),
  JSON.stringify(
    { mcpServers: { fixture: { command: process.execPath, args: [serverPath] } } },
    null,
    2,
  ),
  "utf8",
);
await writeFile(serverPath, MCP_SERVER, "utf8");
await writeFile(
  join(marketplaceRoot, "marketplace.json"),
  JSON.stringify(
    {
      name: "fixture-marketplace",
      owner: { name: "Fixture" },
      plugins: [{ name: "fixture", source: "./plugins/fixture", description: "Fixture MCP plugin" }],
    },
    null,
    2,
  ),
  "utf8",
);

function runCli(args: string[]): string {
  return execFileSync(process.execPath, [TSX_CLI, CLI_ENTRY, ...args], {
    env,
    encoding: "utf8",
    timeout: 60_000,
  });
}

process.stdout.write("\n########## 安装含 MCP 的 fixture 插件 ##########\n");
process.stdout.write(runCli(["plugins", "marketplace", "add", marketplaceRoot, "--scope", "user"]));
process.stdout.write(runCli(["plugins", "install", "fixture@fixture-marketplace", "--scope", "user"]));
const listed = JSON.parse(runCli(["plugins", "list", "--json"]));
const fixture = listed.find((entry: { id?: string }) => entry.id === "fixture@fixture-marketplace");
process.stdout.write(
  `\nfixture entry: declaredMcp=${JSON.stringify(fixture?.declaredMcpServerNames)} mcp=${JSON.stringify(
    fixture?.mcpServerNames,
  )}\n`,
);

const frames: Record<string, unknown>[] = [];
let names: string[] = [];
const session = await startOracleRuntime({
  dataRoot,
  cwd,
  script: [
    {
      toolCalls: [
        {
          name: "mcp__plugin_fixture_fixture__echo",
          input: { text: "hello from mcp" },
        },
      ],
    },
    { text: "mcp probes done" },
  ],
  onMockRequestBody: (_path, body) => {
    if (!Array.isArray(body.tools) || body.tools.length === 0 || names.length > 0) return;
    names = body.tools.map((tool) => String((tool as { name?: unknown }).name ?? ""));
  },
  onRawFrame: (frame) => frames.push(frame),
});

function observed(frame: Record<string, unknown>) {
  if (frame.method !== "session/event") return undefined;
  const params = (frame.params ?? {}) as Record<string, unknown>;
  const type = typeof params.type === "string" ? params.type : undefined;
  const payload =
    typeof params.payload === "object" && params.payload !== null
      ? (params.payload as Record<string, unknown>)
      : {};
  return type ? { type, payload } : undefined;
}

try {
  await session.send("probe mcp fixture");
  const deadline = Date.now() + 60_000;
  while (
    !frames.some((frame) => {
      const event = observed(frame);
      return event?.type === "turn.completed" || event?.type === "turn.failed";
    })
  ) {
    if (Date.now() > deadline) throw new Error("probe：没有终态");
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
} finally {
  await session.stop();
}

process.stdout.write(
  `\ntoolCount=${names.length}\nmcp tools=${JSON.stringify(
    names.filter((name) => name.startsWith("mcp__")),
  )}\n`,
);
for (const frame of frames) {
  const event = observed(frame);
  if (!event) continue;
  if (event.type === "tool.updated" && event.payload.kind === "started") {
    process.stdout.write(
      `started tool=${String(event.payload.toolName)} readOnly=${String(
        event.payload.readOnly,
      )} sideEffectScope=${String(event.payload.sideEffectScope)}\n`,
    );
  }
  if (event.type === "permission.requested") {
    const options = (event.payload.options as { optionId?: string }[] | undefined)?.map(
      (option) => option.optionId,
    );
    process.stdout.write(
      `permission.requested tool=${String(event.payload.toolName)} risk=${String(
        event.payload.riskLevel,
      )} reason=${JSON.stringify(event.payload.reason)} options=${JSON.stringify(options)}\n`,
    );
  }
  if (
    event.type === "tool.updated" &&
    (event.payload.kind === "result" || event.payload.kind === "error")
  ) {
    process.stdout.write(
      `${event.payload.kind} ${String(event.payload.toolCallId)}: ${JSON.stringify(
        event.payload.error ?? event.payload.result,
      )}\n`,
    );
  }
}

await rm(dataRoot, { recursive: true, force: true });
await rm(cwd, { recursive: true, force: true });
await rm(marketplaceRoot, { recursive: true, force: true });
