/* oxlint-disable eslint(max-lines) -- C4 八个观察面共享同一套投影/规范化/DB 读面，拆散会复制规则。 */
// C4 Task 0：生态面（MCP / Skills / Plugins / Workflows / 附件 / v4 帧 / automation /
// 子代理执行）探针的共享支撑。
//
// 与 C2/C3 support 的分工：C2 负责隔离 oracle 与帧录制，C3 负责目标/子会话读面；
// 这里补 C4 需要的四件事：
//   1. 任一方法「调用 + 规范化 + 失败不抛出」的 tryRequest（生态面探针大量观察错误形状）；
//   2. 非 session/event 通知（plugins/operationProgress、v4/conversation/frame 等）的原始捕获
//      （capture 的 IGNORED_NOTIFICATIONS 会丢弃 plugins/operationProgress，C4 要观察它们）；
//   3. 会话外的事实源写入：本地 marketplace、stdio MCP server、工作流脚本文件；
//   4. dwf_* / workflow_* / attachment 落盘等 C4 表与文件的读面。
//
// clean-room：本文件只运行 oracle 并读它写出的帧/行/文件，不读实现体。
import { DatabaseSync } from "node:sqlite";
import { existsSync } from "node:fs";
import { mkdir, readdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import type { JsonObject, PathContext } from "./session-lifecycle-fixtures.mjs";
import { delay } from "./probe-c2-support.mjs";
import {
  createC3Aliases,
  normalizeC3Deep,
  normalizeError as normalizeC3Error,
  readC3Db,
  startC3Harness,
  isolationRecord as isolationC3Record,
  type C3AliasState,
  type C3Harness,
  type C3HarnessOptions,
} from "./probe-c3-support.mjs";

export const C4_FIXTURE_DIR = fileURLToPath(new URL("./fixtures/c4/", import.meta.url));
export { delay, existsSync };

/** C4 需要读的生态表（C3_TABLES 之外）；顺序固定，夹具按 rowid 记录。 */
export const C4_EXTRA_TABLES = [
  "dwf_actor",
  "dwf_event",
  "dwf_node",
  "dwf_run",
  "input_history",
  "local_setting",
  "permission",
  "todo",
  "workflow_activity",
  "workflow_definition",
  "workflow_event",
  "workflow_run",
] as const;

const C4_JSON_COLUMNS: Record<string, readonly string[]> = {
  dwf_actor: ["payload", "last_payload"],
  dwf_event: ["payload"],
  dwf_node: ["payload"],
  dwf_run: ["args_json", "payload"],
  permission: ["data"],
  todo: ["data"],
  session_input: ["payload"],
  session_entry: ["data"],
  workflow_definition: ["meta_json", "args_json"],
  workflow_event: ["payload"],
  workflow_run: ["args_json", "artifacts_json"],
};

export type C4Extras = Record<string, string>;

/** 先按运行时发现的动态值（logEpoch/subscriptionId/uploadId…）做字符串折叠，再走 C3 规范化。 */
function preNormalize(value: unknown, extras: C4Extras): unknown {
  if (typeof value === "string") {
    let text = value;
    for (const [raw, token] of Object.entries(extras)) {
      if (raw.length > 0) text = text.replaceAll(raw, token);
    }
    return text;
  }
  if (Array.isArray(value)) return value.map((item) => preNormalize(item, extras));
  if (value === null || typeof value !== "object") return value;
  return Object.fromEntries(
    Object.entries(value as JsonObject).map(([key, inner]) => [key, preNormalize(inner, extras)]),
  );
}

/** -32603（网关内部抛错）的错误帧 data 里带实现路径 stack；保留可判定字段。 */
function stripErrorFrameStack(error: JsonObject): JsonObject {
  const frame = error.frame;
  if (frame === null || typeof frame !== "object") return error;
  const data = (frame as JsonObject).data;
  if (data === null || typeof data !== "object") return error;
  const { stack: _stack, ...rest } = data as JsonObject;
  return { ...error, frame: { ...(frame as JsonObject), data: rest } };
}

export type C4TryResult = { ok: true; result: JsonObject } | { ok: false; error: JsonObject };

/** C4 harness：C3 读面 + 原始通知捕获 + tryRequest + C4 表读面。 */
export type C4Harness = C3Harness & {
  /** 运行时发现的动态值（logEpoch/subscriptionId…），夹具规范化时折叠。 */
  extras: C4Extras;
  /** 收到的全部非 session/event 通知（method + 原始 params），按到达序。 */
  rawNotifications: { method: string; params: JsonObject }[];
  /** 规范化后的通知投影（extras/别名已折叠）。 */
  notifications(method: string): JsonObject[];
  /** 调用任一方法；成功/失败都规范化，失败不抛出。 */
  tryRequest(method: string, params: Record<string, unknown>, label?: string): Promise<C4TryResult>;
  /** 任意字符串/结构的夹具规范化（extras + 别名 + 机器路径折叠）。 */
  normalizeForFixture(value: unknown): unknown;
  /** workspace ref（探针复用，减少手写）。 */
  workspaceRef(): JsonObject;
  /** 读 C4 生态表（含 C3 全表）。 */
  readC4Tables(tables?: readonly string[]): JsonObject;
  /** 数据根下某目录的文件名清单（附件落盘观察）。 */
  listFiles(relativePath: string): Promise<string[]>;
};

export type C4HarnessOptions = C3HarnessOptions & {
  /** 会话参数覆盖（透传 C2/C3 链到驱动的 sessionCreateParams；MCP/工作流观察用）。 */
  sessionCreateParams?: Record<string, unknown>;
};

/** 起一个隔离 oracle 并在 C3 harness 上补 C4 读面。 */
export async function startC4Harness(options: C4HarnessOptions): Promise<C4Harness> {
  const rawNotifications: { method: string; params: JsonObject }[] = [];
  const harness = await startC3Harness({
    ...options,
    onRawFrame: (frame) => {
      if (
        typeof frame.method === "string" &&
        frame.method !== "session/event" &&
        frame.id === undefined &&
        frame.params !== null &&
        typeof frame.params === "object"
      ) {
        rawNotifications.push({ method: frame.method, params: frame.params as JsonObject });
      }
      options.onRawFrame?.(frame);
    },
  });
  const extras: C4Extras = {};
  const aliasState = harness.aliases;
  const c4: C4Harness = {
    ...harness,
    extras,
    rawNotifications,
    notifications(method: string): JsonObject[] {
      return rawNotifications
        .filter((item) => item.method === method)
        .map((item) => normalizeC4(item.params, harness.context, extras, aliasState) as JsonObject);
    },
    async tryRequest(method, params) {
      try {
        const raw = (await harness.session.request(method, params)) as JsonObject;
        return {
          ok: true,
          result: normalizeC4(raw, harness.context, extras, aliasState) as JsonObject,
        };
      } catch (error) {
        return { ok: false, error: normalizeC4ErrorC4(error, harness.context, extras, aliasState) };
      }
    },
    normalizeForFixture(value) {
      return normalizeC4(value, harness.context, extras, aliasState);
    },
    workspaceRef() {
      return {
        workspacePath: harness.cwd,
        workspaceKey: harness.cwd,
      } as JsonObject;
    },
    readC4Tables(tables) {
      return readC4Db(harness.dbPath, harness.context, aliasState, tables);
    },
    async listFiles(relativePath) {
      const target = join(harness.dataRoot, relativePath);
      if (!existsSync(target)) return [];
      return readdir(target);
    },
  };
  return c4;
}

export function normalizeC4(
  value: unknown,
  context: PathContext,
  extras: C4Extras,
  aliases?: C3AliasState,
): unknown {
  return normalizeC3Deep(preNormalize(value, extras), context, aliases ?? createC3Aliases());
}

export function normalizeC4ErrorC4(
  error: unknown,
  context: PathContext,
  extras: C4Extras,
  aliases?: C3AliasState,
): JsonObject {
  return normalizeC4(
    stripErrorFrameStack(normalizeC3Error(error, context, aliases) as JsonObject),
    context,
    extras,
    aliases,
  ) as JsonObject;
}

/** 读 C4 生态表；JSON 列解析、null 保留，与 readC3Db 同一口径。 */
export function readC4Db(
  path: string,
  context: PathContext,
  aliases?: C3AliasState,
  only?: readonly string[],
): JsonObject {
  const base = readC3Db(path, context, aliases ?? createC3Aliases());
  const tables: JsonObject = { ...(base as JsonObject) };
  const db = new DatabaseSync(path, { readOnly: true });
  try {
    for (const table of C4_EXTRA_TABLES) {
      if (only !== undefined && !only.includes(table)) continue;
      const columns = db.prepare(`pragma table_info(${table})`).all() as JsonObject[];
      const rows = db.prepare(`select * from ${table} order by rowid`).all() as JsonObject[];
      tables[table] = {
        columns: columns.map((column) => ({
          name: column.name,
          type: column.type,
          notNull: column.notnull === 1,
          primaryKey: column.pk,
          defaultValue: column.dflt_value,
        })),
        rowCount: rows.length,
        rows: rows.map((row) => {
          const parsed: JsonObject = { ...row };
          for (const column of C4_JSON_COLUMNS[table] ?? []) {
            if (typeof parsed[column] === "string") {
              try {
                parsed[column] = JSON.parse(parsed[column] as string);
              } catch {
                // 非 JSON 文本列保持原样。
              }
            }
          }
          return normalizeC3Deep(parsed, context, aliases ?? createC3Aliases()) as JsonObject;
        }),
      };
    }
    return tables;
  } finally {
    db.close();
  }
}

export function isolationRecord(harness: C4Harness): JsonObject {
  return isolationC3Record(harness);
}

export async function writeC4Fixture(name: string, fixture: JsonObject): Promise<string> {
  await mkdir(C4_FIXTURE_DIR, { recursive: true });
  const target = fileURLToPath(new URL(`./fixtures/c4/${name}.json`, import.meta.url));
  await writeFile(target, `${JSON.stringify(fixture, null, 2)}\n`, "utf8");
  return target;
}

// ── 会话外事实源写入 ──

/** 最小 stdio MCP server：echo（回显）/ fail（永远失败）/ slow（延迟后回答）。 */
export function buildStdioMcpServerSource(options: {
  serverName: string;
  failMode?: "error_result" | "crash";
}): string {
  const failMode = options.failMode ?? "error_result";
  return [
    'import { createInterface } from "node:readline";',
    "const rl = createInterface({ input: process.stdin });",
    "const send = (value) => process.stdout.write(JSON.stringify(value) + '\\n');",
    "rl.on('line', (line) => {",
    "  let message; try { message = JSON.parse(line); } catch { return; }",
    "  if (message.id === undefined) return;",
    `  if (message.method === 'initialize') { send({ jsonrpc: '2.0', id: message.id, result: { protocolVersion: '2024-11-05', capabilities: { tools: {} }, serverInfo: { name: '${options.serverName}', version: '1.0.0' } } }); return; }`,
    "  if (message.method === 'notifications/initialized') return;",
    "  if (message.method === 'ping') { send({ jsonrpc: '2.0', id: message.id, result: {} }); return; }",
    "  if (message.method === 'tools/list') {",
    "    send({ jsonrpc: '2.0', id: message.id, result: { tools: [",
    "      { name: 'echo', description: 'Echo the given text back.', inputSchema: { type: 'object', properties: { text: { type: 'string' } }, required: ['text'], additionalProperties: false } },",
    "      { name: 'fail', description: 'Always fails on purpose.', inputSchema: { type: 'object', properties: {}, additionalProperties: false } },",
    "      { name: 'slow', description: 'Replies after a delay.', inputSchema: { type: 'object', properties: { ms: { type: 'number' } }, additionalProperties: false } }",
    "    ] } }); return;",
    "  }",
    "  if (message.method === 'tools/call') {",
    "    const name = message.params && message.params.name;",
    `    if (name === 'fail') {`,
    `      if ('${failMode}' === 'crash') { process.exit(3); }`,
    "      send({ jsonrpc: '2.0', id: message.id, result: { isError: true, content: [{ type: 'text', text: 'probe-mcp deliberate failure' }] } }); return;",
    "    }",
    "    if (name === 'slow') {",
    "      const ms = Number((message.params && message.params.arguments && message.params.arguments.ms) || 400);",
    "      setTimeout(() => send({ jsonrpc: '2.0', id: message.id, result: { content: [{ type: 'text', text: 'slow reply after ' + ms + 'ms' }] } }), ms);",
    "      return;",
    "    }",
    "    const text = String((message.params && message.params.arguments && message.params.arguments.text) || '');",
    "    send({ jsonrpc: '2.0', id: message.id, result: { content: [{ type: 'text', text: 'echo:' + text }] } });",
    "    return;",
    "  }",
    "  send({ jsonrpc: '2.0', id: message.id, error: { code: -32601, message: 'method not found: ' + message.method } });",
    "});",
    "",
  ].join("\n");
}

/** 把 MCP server 脚本写到隔离根下（cwd 不参与，路径由探针传给 session/create）。 */
export async function writeStdioMcpServer(
  dataRoot: string,
  name: string,
  options: { serverName: string; failMode?: "error_result" | "crash" },
): Promise<string> {
  const target = join(dataRoot, "c4-fixtures", `${name}-mcp-server.mjs`);
  await mkdir(join(target, ".."), { recursive: true });
  await writeFile(target, buildStdioMcpServerSource(options), "utf8");
  return target;
}

/** 本地 marketplace + fixture 插件（.mode-plugin/plugin.json + commands/ + skills/ + 可选 .mcp.json）。 */
export async function writeLocalMarketplace(
  marketplaceRoot: string,
  options: { pluginName: string; withMcp?: boolean; withSkill?: boolean; withCommand?: boolean },
): Promise<{ pluginRoot: string; mcpServerPath: string | null }> {
  const pluginRoot = join(marketplaceRoot, "plugins", options.pluginName);
  await mkdir(join(pluginRoot, ".mode-plugin"), { recursive: true });
  await writeFile(
    join(pluginRoot, ".mode-plugin", "plugin.json"),
    JSON.stringify(
      {
        name: options.pluginName,
        version: "1.0.0",
        description: `C4 probe fixture plugin ${options.pluginName}`,
      },
      null,
      2,
    ),
    "utf8",
  );
  let mcpServerPath: string | null = null;
  if (options.withMcp) {
    mcpServerPath = join(pluginRoot, "mcp-server.mjs");
    await writeFile(
      mcpServerPath,
      buildStdioMcpServerSource({ serverName: options.pluginName }),
      "utf8",
    );
    await writeFile(
      join(pluginRoot, ".mcp.json"),
      JSON.stringify(
        {
          mcpServers: {
            [options.pluginName]: { command: process.execPath, args: [mcpServerPath] },
          },
        },
        null,
        2,
      ),
      "utf8",
    );
  }
  if (options.withSkill) {
    await mkdir(join(pluginRoot, "skills", `${options.pluginName}-skill`), { recursive: true });
    await writeFile(
      join(pluginRoot, "skills", `${options.pluginName}-skill`, "SKILL.md"),
      [
        "---",
        `name: ${options.pluginName}-skill`,
        "description: Use when the C4 probe asks to exercise the fixture plugin skill.",
        "---",
        "",
        `${options.pluginName} fixture skill body.`,
        "",
      ].join("\n"),
      "utf8",
    );
  }
  if (options.withCommand) {
    await mkdir(join(pluginRoot, "commands"), { recursive: true });
    await writeFile(
      join(pluginRoot, "commands", "hello.md"),
      ["---", "description: Fixture command", "---", "Hello from the C4 fixture plugin.", ""].join(
        "\n",
      ),
      "utf8",
    );
  }
  await writeFile(
    join(marketplaceRoot, "marketplace.json"),
    JSON.stringify(
      {
        name: `${options.pluginName}-marketplace`,
        owner: { name: "C4 Probe" },
        plugins: [
          {
            name: options.pluginName,
            source: "./plugins/" + options.pluginName,
            description: `C4 probe fixture plugin ${options.pluginName}`,
          },
        ],
      },
      null,
      2,
    ),
    "utf8",
  );
  return { pluginRoot, mcpServerPath };
}

/** CLI 面的 marketplace/install（与 probe-plugin-mcp.mts 同一模式，输出原文返回）。 */
export async function runOracleCli(
  repoRoot: string,
  env: NodeJS.ProcessEnv,
  args: string[],
): Promise<string> {
  const { execFileSync } = await import("node:child_process");
  const tsxCli = join(repoRoot, "node_modules", "tsx", "dist", "cli.mjs");
  const cliEntry = join(repoRoot, "apps", "mode-cli", "packages", "cli", "src", "main.ts");
  return execFileSync(process.execPath, [tsxCli, cliEntry, ...args], {
    env,
    encoding: "utf8",
    timeout: 120_000,
  });
}

/** 等条件成立（C2 waitFor 的转发，避免探针多一个 import）。 */
export { waitForTerminals, waitFor } from "./probe-c2-support.mjs";
