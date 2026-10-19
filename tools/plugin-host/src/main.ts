// 插件宿主 sidecar（M3 生态域 1 · Task 3–5）。
//
// 运行（开发形态）：
//   pnpm exec tsx tools/plugin-host/src/main.ts
//
// 协议：一行一帧 NDJSON，与 `crates/plugin-host/src/protocol.rs` 一一对应。
//
// 发现：扫描 `<dataRoot>/.mode/cli/plugins/cache/<marketplace>/<plugin>/<version>/`，
// 读取 `.mode-plugin/plugin.json` 与 `.mcp.json` 的 `mcpServers`，按需启动 MCP 服务器。
// 工具名：`mcp__plugin_<plugin>_<server>__<tool>`（观察值）。
// 元数据：插件 MCP 默认 medium/network/truncate；`node-repl-host` 是
// high/system/artifact（观察值）。
//
// 未做：启用/禁用过滤（缺 installed_plugins.json 的稳定读取口径）、Skills（Skill 是内置工具）。
import { MODE_DATA_ROOT_DIR_NAME } from "@mode/shared";
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { readdir, readFile } from "node:fs/promises";
import { createInterface } from "node:readline";
import { join } from "node:path";

const PROTOCOL_VERSION = 1;
const PROTOCOL_ERROR_ID = 0;
const REQUEST_TIMEOUT_MS = 20_000;

type ToolMetadata = {
  riskLevel: string;
  sideEffectScope: string;
  budgetStrategy: string;
};

type ToolDeclaration = {
  name: string;
  description: string;
  inputSchema: Record<string, unknown>;
} & ToolMetadata;

type ServerConfig = {
  command: string;
  args?: string[];
  env?: Record<string, string>;
};

type DiscoveredServer = {
  plugin: string;
  server: string;
  root: string;
  config: ServerConfig;
  metadata: ToolMetadata;
  client: McpClient;
  tools: ToolDeclaration[] | null;
};

function write(frame: unknown): void {
  process.stdout.write(`${JSON.stringify(frame)}\n`);
}

function ok(id: number, result: unknown): void {
  write({ id, result });
}

function fail(id: number, code: number, message: string): void {
  write({ id, error: { code, message } });
}

function pluginStorageBase(): string {
  return (
    process.env.MODE_DATA_BASE_DIR?.trim() ||
    process.env.MODE_HOME?.trim() ||
    process.env.HOME ||
    process.env.USERPROFILE ||
    ""
  );
}

function safeToken(value: string): string {
  return value.replace(/[^A-Za-z0-9]/g, "_");
}

async function discoverServers(): Promise<DiscoveredServer[]> {
  const base = pluginStorageBase();
  const cacheRoot = join(base, MODE_DATA_ROOT_DIR_NAME, "cli", "plugins", "cache");
  let enabledPlugins: Record<string, boolean> = {};
  try {
    const config = JSON.parse(
      await readFile(join(base, MODE_DATA_ROOT_DIR_NAME, "cli", "config.json"), "utf8"),
    ) as { plugins?: { enabledPlugins?: Record<string, boolean> } };
    enabledPlugins = config.plugins?.enabledPlugins ?? {};
  } catch {
    // 没有 config 时按默认启用处理（与 CLI 的默认集合一致的部分由此覆盖）。
  }
  const discovered: DiscoveredServer[] = [];
  let marketplaces;
  try {
    marketplaces = await readdir(cacheRoot, { withFileTypes: true });
  } catch {
    return discovered;
  }
  for (const marketplace of marketplaces.filter((entry) => entry.isDirectory())) {
    let plugins;
    try {
      plugins = await readdir(join(cacheRoot, marketplace.name), { withFileTypes: true });
    } catch {
      continue;
    }
    for (const plugin of plugins.filter((entry) => entry.isDirectory())) {
      let versions;
      try {
        versions = await readdir(join(cacheRoot, marketplace.name, plugin.name), {
          withFileTypes: true,
        });
      } catch {
        continue;
      }
      const versionNames = versions
        .filter((entry) => entry.isDirectory())
        .map((entry) => entry.name)
        .sort((left, right) => right.localeCompare(left, undefined, { numeric: true }));
      const version = versionNames[0];
      if (version === undefined) continue;
      const root = join(cacheRoot, marketplace.name, plugin.name, version);
      let manifest: { name?: string };
      try {
        manifest = JSON.parse(await readFile(join(root, ".mode-plugin", "plugin.json"), "utf8"));
      } catch {
        continue;
      }
      const pluginName = manifest.name ?? plugin.name;
      const pluginId = `${pluginName}@${marketplace.name}`;
      const enabled = enabledPlugins[pluginId];
      if (enabled === false) continue;
      // 官方内置里默认关闭的 computer-use 没有显式配置时跳过；其余无配置按启用。
      if (enabled === undefined && pluginName === "computer-use") continue;
      // 内置 node-repl 宿主不是用户插件；它的工具是 `mcp__node_repl__js`，由内置面提供。
      if (pluginName === "node-repl-host") continue;
      let mcp: { mcpServers?: Record<string, ServerConfig> };
      try {
        mcp = JSON.parse(await readFile(join(root, ".mcp.json"), "utf8"));
      } catch {
        continue;
      }
      const metadata: ToolMetadata = {
        riskLevel: "medium",
        sideEffectScope: "network",
        budgetStrategy: "truncate",
      };
      for (const [serverName, config] of Object.entries(mcp.mcpServers ?? {})) {
        discovered.push({
          plugin: pluginName,
          server: serverName,
          root,
          config,
          metadata,
          client: new McpClient(config, root),
          tools: null,
        });
      }
    }
  }
  return discovered;
}

class McpClient {
  private child: ChildProcessWithoutNullStreams | null = null;
  private readonly pending = new Map<
    number,
    { resolve: (value: unknown) => void; reject: (error: Error) => void }
  >();
  private nextId = 1;

  constructor(
    private readonly config: ServerConfig,
    private readonly cwd: string,
  ) {}

  shutdown(): void {
    if (this.child !== null) {
      this.child.kill();
      this.child = null;
    }
  }

  async listTools(): Promise<Array<{ name: string; description?: string; inputSchema?: Record<string, unknown> }>> {
    const result = (await this.request("tools/list", {})) as {
      tools?: Array<{ name: string; description?: string; inputSchema?: Record<string, unknown> }>;
    };
    return result.tools ?? [];
  }

  async callTool(name: string, args: Record<string, unknown>): Promise<string> {
    const result = (await this.request("tools/call", { name, arguments: args })) as {
      content?: Array<{ type?: string; text?: string }>;
    };
    return (result.content ?? [])
      .map((part) => (part.type === "text" ? String(part.text ?? "") : ""))
      .join("");
  }

  private async ensureStarted(): Promise<void> {
    if (this.child !== null) return;
    const child = spawn(this.config.command, this.config.args ?? [], {
      cwd: this.cwd,
      env: { ...process.env, ...this.config.env },
      stdio: ["pipe", "pipe", "pipe"],
    });
    this.child = child;
    const lines = createInterface({ input: child.stdout });
    lines.on("line", (line) => this.onLine(line));
    child.on("exit", () => {
      this.child = null;
      for (const request of this.pending.values()) {
        request.reject(new Error("Connection closed"));
      }
      this.pending.clear();
    });
    await this.request("initialize", {
      protocolVersion: "2024-11-05",
      capabilities: {},
      clientInfo: { name: "mode-plugin-host", version: "1.0.0" },
    });
    this.notify("notifications/initialized", {});
  }

  private request(method: string, params: unknown): Promise<unknown> {
    return this.ensureStarted().then(
      () =>
        new Promise<unknown>((resolve, reject) => {
          const id = this.nextId++;
          this.pending.set(id, { resolve, reject });
          this.child?.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
          const timer = setTimeout(() => {
            this.pending.delete(id);
            reject(new Error(`timeout: ${method}`));
          }, REQUEST_TIMEOUT_MS);
          const original = this.pending.get(id);
          if (original) {
            this.pending.set(id, {
              resolve: (value) => {
                clearTimeout(timer);
                original.resolve(value);
              },
              reject: (error) => {
                clearTimeout(timer);
                original.reject(error);
              },
            });
          }
        }),
    );
  }

  private notify(method: string, params: unknown): void {
    this.child?.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", method, params })}\n`);
  }

  private onLine(line: string): void {
    if (line.trim() === "") return;
    let message: { id?: number; result?: unknown; error?: { message?: string } };
    try {
      message = JSON.parse(line);
    } catch {
      return;
    }
    if (typeof message.id !== "number") return;
    const request = this.pending.get(message.id);
    if (!request) return;
    this.pending.delete(message.id);
    if (message.error) {
      request.reject(new Error(message.error.message ?? "mcp error"));
    } else {
      request.resolve(message.result);
    }
  }
}

let servers = await discoverServers();

async function refreshServers(): Promise<void> {
  for (const server of servers) server.client.shutdown();
  servers = await discoverServers();
}

async function toolsFor(server: DiscoveredServer): Promise<ToolDeclaration[]> {
  if (server.tools !== null) return server.tools;
  try {
    const listed = await server.client.listTools();
    server.tools = listed.map((tool) => ({
      name: `mcp__plugin_${safeToken(server.plugin)}_${safeToken(server.server)}__${tool.name}`,
      description: tool.description ?? "",
      inputSchema: tool.inputSchema ?? { type: "object" },
      ...server.metadata,
    }));
  } catch {
    // 坏 server 的降级：工具静默缺失（观察：broken 插件不在工具清单里）。
    server.tools = [];
  }
  return server.tools;
}

async function allTools(): Promise<ToolDeclaration[]> {
  const groups = await Promise.all(servers.map((server) => toolsFor(server)));
  return groups.flat();
}

const input = createInterface({ input: process.stdin });
input.on("line", (line) => {
  if (line.trim() === "") return;
  let frame: Record<string, unknown>;
  try {
    frame = JSON.parse(line) as Record<string, unknown>;
  } catch {
    fail(PROTOCOL_ERROR_ID, -32700, "parse error");
    return;
  }
  const id = typeof frame.id === "number" ? frame.id : undefined;
  const method = typeof frame.method === "string" ? frame.method : undefined;
  if (method === undefined || id === undefined) {
    fail(id ?? PROTOCOL_ERROR_ID, -32600, "invalid request");
    return;
  }
  const params = (frame.params ?? {}) as Record<string, unknown>;
  void (async () => {
    try {
      switch (method) {
        case "host/hello":
          ok(id, { protocolVersion: PROTOCOL_VERSION, servers: servers.length });
          return;
        case "plugin/refresh":
          await refreshServers();
          ok(id, { servers: servers.length });
          return;
        case "plugin/listTools":
          if (params.refresh === true) await refreshServers();
          ok(id, { tools: await allTools() });
          return;
        case "plugin/health":
          ok(id, { ok: true, servers: servers.length });
          return;
        case "plugin/callTool": {
          const toolName = String(params.toolName ?? "");
          const inputValue = (params.input ?? {}) as Record<string, unknown>;
          for (const server of servers) {
            const tools = await toolsFor(server);
            const tool = tools.find((candidate) => candidate.name === toolName);
            if (tool === undefined) continue;
            const content = await server.client.callTool(
              toolName.replace(
                `mcp__plugin_${safeToken(server.plugin)}_${safeToken(server.server)}__`,
                "",
              ),
              inputValue,
            );
            ok(id, { content });
            return;
          }
          fail(id, -32601, `Tool not found: ${toolName}`);
          return;
        }
        case "host/shutdown":
          ok(id, {});
          setImmediate(() => process.exit(0));
          return;
        default:
          fail(id, -32601, `method not found: ${method}`);
      }
    } catch (error) {
      fail(id, -32000, error instanceof Error ? error.message : String(error));
    }
  })();
});
