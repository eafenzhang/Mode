// M3 插件宿主 Task 5 Step 3：MCP 生命周期观察（启动耗时、崩溃、坏 server 降级）。
//
// 用法：
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-plugin-mcp-lifecycle.mts
//
// 两个插件：fixture（echo + crash 工具）、broken（command 指向不存在的文件）。
// 只观察行为，不读实现体。
import { execFileSync } from "node:child_process";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { startOracleRuntime } from "./driver.mjs";

const REPO_ROOT = fileURLToPath(new URL("../../../../", import.meta.url));
const TSX_CLI = join(REPO_ROOT, "node_modules", "tsx", "dist", "cli.mjs");
const CLI_ENTRY = join(REPO_ROOT, "apps", "mode-cli", "packages", "cli", "src", "main.ts");

const dataRoot = await mkdtemp(join(tmpdir(), "mode-probe-mcp-life-"));
const cwd = await mkdtemp(join(tmpdir(), "mode-probe-mcp-life-ws-"));
const marketplaceRoot = await mkdtemp(join(tmpdir(), "mode-probe-mcp-life-market-"));
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
  "    send({ jsonrpc: '2.0', id: message.id, result: { tools: [",
  "      { name: 'echo', description: 'Echo text', inputSchema: { type: 'object', properties: { text: { type: 'string' } }, required: ['text'], additionalProperties: false } },",
  "      { name: 'crash', description: 'Exit the server', inputSchema: { type: 'object', properties: {}, additionalProperties: false } }",
  "    ] } });",
  "    return;",
  "  }",
  "  if (message.method === 'tools/call') {",
  "    const name = message.params && message.params.name;",
  "    if (name === 'crash') { process.exit(0); return; }",
  "    const text = (message.params && message.params.arguments && message.params.arguments.text) || '';",
  "    send({ jsonrpc: '2.0', id: message.id, result: { content: [{ type: 'text', text: String(text) }] } });",
  "    return;",
  "  }",
  "  if (message.method === 'ping') { send({ jsonrpc: '2.0', id: message.id, result: {} }); return; }",
  "  send({ jsonrpc: '2.0', id: message.id, error: { code: -32601, message: 'method not found: ' + message.method } });",
  "});",
  "",
].join("\n");

async function writePlugin(name: string, mcpServer: string) {
  const root = join(marketplaceRoot, "plugins", name);
  await mkdir(join(root, ".mode-plugin"), { recursive: true });
  await writeFile(
    join(root, ".mode-plugin", "plugin.json"),
    JSON.stringify({ name, version: "1.0.0", description: `${name} plugin` }, null, 2),
    "utf8",
  );
  await writeFile(
    join(root, ".mcp.json"),
    JSON.stringify({ mcpServers: { [name]: mcpServer } }, null, 2),
    "utf8",
  );
  return root;
}

await writePlugin("fixture", {
  command: process.execPath,
  args: [join(marketplaceRoot, "mcp-server.mjs")],
});
await writeFile(join(marketplaceRoot, "mcp-server.mjs"), MCP_SERVER, "utf8");
await writePlugin("broken", { command: process.execPath, args: [join(marketplaceRoot, "missing-server.mjs")] });

await writeFile(
  join(marketplaceRoot, "marketplace.json"),
  JSON.stringify(
    {
      name: "fixture-marketplace",
      owner: { name: "Fixture" },
      plugins: [
        { name: "fixture", source: "./plugins/fixture", description: "Fixture plugin" },
        { name: "broken", source: "./plugins/broken", description: "Broken plugin" },
      ],
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

process.stdout.write("\n########## 安装 fixture + broken 插件 ##########\n");
process.stdout.write(runCli(["plugins", "marketplace", "add", marketplaceRoot, "--scope", "user"]));
process.stdout.write(runCli(["plugins", "install", "fixture@fixture-marketplace", "--scope", "user"]));
process.stdout.write(runCli(["plugins", "install", "broken@fixture-marketplace", "--scope", "user"]));
const listed = JSON.parse(runCli(["plugins", "list", "--json"]));
for (const id of ["fixture@fixture-marketplace", "broken@fixture-marketplace"]) {
  const entry = listed.find((item: { id?: string }) => item.id === id);
  process.stdout.write(
    `${id}: declaredMcp=${JSON.stringify(entry?.declaredMcpServerNames)} mcp=${JSON.stringify(
      entry?.mcpServerNames,
    )} diagnostics=${JSON.stringify(entry?.diagnostics)}\n`,
  );
}

const frames: Record<string, unknown>[] = [];
let names: string[] = [];
const startedAt = Date.now();
const session = await startOracleRuntime({
  dataRoot,
  cwd,
  script: [
    { toolCalls: [{ name: "mcp__plugin_fixture_fixture__crash", input: {} }] },
    { toolCalls: [{ name: "mcp__plugin_fixture_fixture__echo", input: { text: "after crash" } }] },
    { text: "mcp lifecycle probes done" },
  ],
  onMockRequestBody: (_path, body) => {
    if (!Array.isArray(body.tools) || body.tools.length === 0 || names.length > 0) return;
    names = body.tools.map((tool) => String((tool as { name?: unknown }).name ?? ""));
  },
  onRawFrame: (frame) => frames.push(frame),
});
const startupMs = Date.now() - startedAt;

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
  await session.send("probe mcp lifecycle");
  const deadline = Date.now() + 90_000;
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

process.stdout.write(`\nstartupMs=${startupMs}\ntoolCount=${names.length}\nmcp tools=${JSON.stringify(
  names.filter((name) => name.startsWith("mcp__")),
)}\n`);
for (const frame of frames) {
  const event = observed(frame);
  if (!event) continue;
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
