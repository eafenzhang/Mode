// M3 插件宿主 Task 3–5：sidecar 真实扫描插件缓存并桥接 MCP 工具。
//
// 用法：
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-plugin-sidecar.mts
//
// 自包含：在临时数据根的插件 cache 放一个 fixture 插件（.mcp.json + 最小 MCP server），
// 启动 tools/plugin-host/src/main.ts，依次发 hello / listTools / callTool / shutdown。
import { spawn } from "node:child_process";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";

const REPO_ROOT = fileURLToPath(new URL("../../../../", import.meta.url));
const TSX_CLI = join(REPO_ROOT, "node_modules", "tsx", "dist", "cli.mjs");
const SIDECAR_ENTRY = join(REPO_ROOT, "tools", "plugin-host", "src", "main.ts");

const dataRoot = await mkdtemp(join(tmpdir(), "mode-probe-sidecar-"));
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
await mkdir(join(pluginRoot, ".tutto-plugin"), { recursive: true });
await writeFile(
  join(pluginRoot, ".tutto-plugin", "plugin.json"),
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
    "  if (message.method === 'tools/list') { send({ jsonrpc: '2.0', id: message.id, result: { tools: [{ name: 'echo', description: 'Echo text', inputSchema: { type: 'object', properties: { text: { type: 'string' } }, required: ['text'] } }] } }); return; }",
    "  if (message.method === 'tools/call') { const text = (message.params && message.params.arguments && message.params.arguments.text) || ''; send({ jsonrpc: '2.0', id: message.id, result: { content: [{ type: 'text', text: String(text) }] } }); return; }",
    "  send({ jsonrpc: '2.0', id: message.id, error: { code: -32601, message: 'method not found' } });",
    "});",
    "",
  ].join("\n"),
  "utf8",
);

// 默认跑 tsx 源码；MODE_PLUGIN_HOST_BUNDLE 指向已构建的 bundle 时改跑 bundle。
const bundleEntry = process.env.MODE_PLUGIN_HOST_BUNDLE?.trim();
const child = bundleEntry
  ? spawn(process.execPath, [bundleEntry], {
      env: { ...process.env, MODE_DATA_BASE_DIR: dataRoot },
      stdio: ["pipe", "pipe", "inherit"],
    })
  : spawn(process.execPath, [TSX_CLI, SIDECAR_ENTRY], {
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

async function request(frame: Record<string, unknown>): Promise<unknown> {
  child.stdin.write(`${JSON.stringify(frame)}\n`);
  const line = await nextLine();
  return JSON.parse(line);
}

try {
  process.stdout.write("\n########## sidecar 真实插件发现 ##########\n");
  process.stdout.write(`hello: ${JSON.stringify(await request({ id: 1, method: "host/hello", params: {} }))}\n`);
  const listed = (await request({ id: 2, method: "plugin/listTools", params: {} })) as {
    result?: { tools?: Array<{ name?: string }> };
  };
  process.stdout.write(
    `tools: ${JSON.stringify(listed.result?.tools?.map((tool) => tool.name))}\n`,
  );
  const called = await request({
    id: 3,
    method: "plugin/callTool",
    params: {
      toolName: "mcp__plugin_fixture_fixture__echo",
      input: { text: "hello from sidecar" },
    },
  });
  process.stdout.write(`callTool: ${JSON.stringify(called)}\n`);
  await request({ id: 4, method: "host/shutdown", params: {} });
} finally {
  child.kill();
  await rm(dataRoot, { recursive: true, force: true });
}
