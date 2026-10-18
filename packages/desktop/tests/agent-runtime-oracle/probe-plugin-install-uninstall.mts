// M3 插件宿主 Task 6：运行中安装/卸载 → `plugin/refresh` → 工具清单变化。
//
// 用法：
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-plugin-install-uninstall.mts
import { spawn } from "node:child_process";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";

const REPO_ROOT = fileURLToPath(new URL("../../../../", import.meta.url));
const TSX_CLI = join(REPO_ROOT, "node_modules", "tsx", "dist", "cli.mjs");
const SIDECAR_ENTRY = join(REPO_ROOT, "tools", "plugin-host", "src", "main.ts");

const dataRoot = await mkdtemp(join(tmpdir(), "mode-probe-install-"));
const pluginRoot = join(
  dataRoot,
  ".mode",
  "cli",
  "plugins",
  "cache",
  "fixture-marketplace",
  "fixture",
  "1.0.0",
);
const serverPath = join(pluginRoot, "mcp-server.mjs");

async function installPlugin(): Promise<void> {
  await mkdir(join(pluginRoot, ".mode-plugin"), { recursive: true });
  await writeFile(
    join(pluginRoot, ".mode-plugin", "plugin.json"),
    JSON.stringify({ name: "fixture", version: "1.0.0", description: "Fixture" }, null, 2),
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
  await writeFile(
    serverPath,
    [
      'import { createInterface } from "node:readline";',
      "const rl = createInterface({ input: process.stdin });",
      "const send = (value) => process.stdout.write(JSON.stringify(value) + '\\n');",
      "rl.on('line', (line) => {",
      "  let message; try { message = JSON.parse(line); } catch { return; }",
      "  if (message.id === undefined) return;",
      "  if (message.method === 'initialize') { send({ jsonrpc: '2.0', id: message.id, result: { protocolVersion: '2024-11-05', capabilities: { tools: {} }, serverInfo: { name: 'fixture', version: '1.0.0' } } }); return; }",
      "  if (message.method === 'tools/list') { send({ jsonrpc: '2.0', id: message.id, result: { tools: [{ name: 'echo', description: 'Echo', inputSchema: { type: 'object', properties: { text: { type: 'string' } }, required: ['text'] } }] } }); return; }",
      "  if (message.method === 'tools/call') { send({ jsonrpc: '2.0', id: message.id, result: { content: [{ type: 'text', text: String((message.params && message.params.arguments && message.params.arguments.text) || '') }] } }); return; }",
      "  send({ jsonrpc: '2.0', id: message.id, error: { code: -32601, message: 'method not found' } });",
      "});",
      "",
    ].join("\n"),
    "utf8",
  );
}

const child = spawn(process.execPath, [TSX_CLI, SIDECAR_ENTRY], {
  env: { ...process.env, MODE_DATA_BASE_DIR: dataRoot },
  stdio: ["pipe", "pipe", "inherit"],
});
const lines = createInterface({ input: child.stdout });
const queue: string[] = [];
const waiters: Array<(line: string) => void> = [];
lines.on("line", (line) => {
  const waiter = waiters.shift();
  if (waiter) waiter(line);
  else queue.push(line);
});

function nextLine(timeoutMs = 20_000): Promise<string> {
  const queued = queue.shift();
  if (queued !== undefined) return Promise.resolve(queued);
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("sidecar 响应超时")), timeoutMs);
    waiters.push((line) => {
      clearTimeout(timer);
      resolve(line);
    });
  });
}

let nextId = 1;
async function request(method: string, params: unknown = {}): Promise<Record<string, unknown>> {
  const id = nextId++;
  child.stdin.write(`${JSON.stringify({ id, method, params })}\n`);
  return JSON.parse(await nextLine()) as Record<string, unknown>;
}

async function toolNames(refresh: boolean): Promise<string[]> {
  if (refresh) await request("plugin/refresh");
  const result = (await request("plugin/listTools")) as {
    result?: { tools?: Array<{ name?: string }> };
  };
  return (result.result?.tools ?? []).map((tool) => String(tool.name));
}

try {
  process.stdout.write("\n########## 安装前 ##########\n");
  process.stdout.write(`tools=${JSON.stringify(await toolNames(false))}\n`);

  await installPlugin();
  process.stdout.write("\n########## 安装后（refresh）##########\n");
  process.stdout.write(`tools=${JSON.stringify(await toolNames(true))}\n`);
  const called = await request("plugin/callTool", {
    toolName: "mcp__plugin_fixture_fixture__echo",
    input: { text: "installed" },
  });
  process.stdout.write(`callTool=${JSON.stringify(called)}\n`);

  // 先 refresh 让 sidecar 杀掉 MCP 子进程（它的 cwd 是插件目录），再删目录。
  await request("plugin/refresh");
  await rm(pluginRoot, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  process.stdout.write("\n########## 卸载后（refresh）##########\n");
  process.stdout.write(`tools=${JSON.stringify(await toolNames(true))}\n`);

  await request("host/shutdown");
} finally {
  child.kill();
  await rm(dataRoot, { recursive: true, force: true });
}
