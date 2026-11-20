// C2 Task 0：compact / memory / contextUsage / fresh-install 探针的共享支撑。
//
// 只做三件事：起隔离 oracle、读库并规范化、写夹具。观察逻辑留在各自的 `probe-c2-*.mts`，
// 避免把「测什么」混进「怎么投影」。clean-room：不读旧实现，只读它写出的帧/行/文件。
import { DatabaseSync } from "node:sqlite";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import {
  seedWorkspace,
  startOracleRuntime,
  type OracleRuntimeOptions,
  type OracleSession,
} from "./driver.mjs";
import type { MockStep } from "./mock-provider.mjs";
import {
  capture,
  normalizeDeep,
  readDb,
  recordFrames,
  type Capture,
  type JsonObject,
  type PathContext,
} from "./session-lifecycle-fixtures.mjs";

export const C2_FIXTURE_DIR = fileURLToPath(new URL("./fixtures/c2/", import.meta.url));
export const TERMINAL_EVENT_TYPES = new Set(["turn.completed", "turn.failed"]);

export function delay(ms: number): Promise<void> {
  return new Promise((resolvePromise) => setTimeout(resolvePromise, ms));
}

export function terminalCount(events: readonly JsonObject[]): number {
  return events.filter((event) => TERMINAL_EVENT_TYPES.has(String(event.type))).length;
}

/** 等到第 `expected` 个 turn 终态；再留 500ms 收尾窗（与 C1 探针同一口径）。 */
export async function waitForTerminals(
  captureState: Capture,
  expected: number,
  timeoutMs = 180_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (terminalCount(captureState.events) < expected) {
    if (Date.now() > deadline) {
      throw new Error(
        `oracle 在 ${timeoutMs}ms 内只产生 ${terminalCount(captureState.events)} 个终态`,
      );
    }
    await delay(50);
  }
  await delay(500);
}

/** 轮询直到条件成立；超时抛错，不静默当成功。 */
export async function waitFor(
  predicate: () => boolean,
  what: string,
  timeoutMs = 60_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error(`oracle 在 ${timeoutMs}ms 内没有观察到 ${what}`);
    await delay(20);
  }
}

export function findRequest(requests: readonly JsonObject[], method: string): JsonObject {
  const found = requests.find((request) => request.method === method);
  if (found === undefined) throw new Error(`没有观察到 ${method} 的请求/应答`);
  return found;
}

/** 机器路径/id 折叠；`extra` 用于 workspaceId 这类运行时发现的值。 */
export function normalizeC2String(
  value: string,
  context: PathContext,
  extra: Record<string, string> = {},
): string {
  let text = value;
  for (const [raw, token] of [
    [context.dataRoot, "<data-root>"],
    [context.cwd, "<cwd>"],
    [context.tmpDir, "<tmp>"],
    ...Object.entries(extra),
  ] as const) {
    text = text.replaceAll(raw, token);
  }
  return text
    .replace(/sess_[0-9a-f-]{36}/giu, "<session-id>")
    .replace(/msg_(?:part_)?[a-z0-9]+_[0-9a-f-]{36}(?:_message)?/giu, "<message-id>")
    .replace(/part_(?:part_)?[a-z0-9]+_[0-9a-f-]{36}(?:_timeline)?/giu, "<part-id>")
    .replace(/turn_[0-9a-f-]{36}/giu, "<turn-id>")
    .replace(/cmp_[0-9a-f-]{36}/giu, "<compact-operation-id>")
    .replace(/compact_[0-9a-f-]{36}/giu, "<compact-boundary-id>")
    .replace(/\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/giu, "<id>")
    .replace(/\b(?:https?):\/\/(?:127\.0\.0\.1|localhost|\[::1\]):\d+/gu, "<loopback-url>")
    .replace(/\b\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z\b/gu, "<time>")
    .replace(/Today's date is \d{4}-\d{2}-\d{2}/gu, "Today's date is <date>");
}

/** 错误事件里的 `stack` 带实现路径且是诊断噪声；保留可判定的错误字段。 */
export function stripErrorStack(payload: unknown): unknown {
  if (payload === null || typeof payload !== "object") return payload;
  const value = payload as JsonObject;
  const error = value.error;
  if (error === null || typeof error !== "object") return payload;
  const { stack: _stack, ...rest } = error as JsonObject;
  return { ...value, error: rest };
}

const C2_TABLES = [
  "message",
  "part",
  "session",
  "session_entry",
  "session_input",
  "tool_usage",
  "turn_usage",
  "model_usage",
] as const;

const JSON_COLUMNS: Record<string, readonly string[]> = {
  model_usage: ["raw_usage_json", "provider_metadata_json"],
};

/**
 * 读 C2 需要的库表：C1 的 5 张表复用 `readDb` 的同一套解析/规范化，
 * usage 三表按同一口径补读（JSON 列解析成对象，id/时间/路径折叠）。
 */
export function readC2Db(path: string, context: PathContext): JsonObject {
  const c1 = readDb(path, context);
  const tables = { ...(c1.tables as JsonObject) };
  const db = new DatabaseSync(path, { readOnly: true });
  try {
    for (const table of C2_TABLES) {
      if (tables[table] !== undefined) continue;
      const columns = db.prepare(`pragma table_info(${table})`).all() as JsonObject[];
      const messageSequences = new Map<string, number>();
      if (table === "part") {
        const messages = db.prepare("select id, sequence from message").all() as JsonObject[];
        for (const message of messages) {
          messageSequences.set(String(message.id), Number(message.sequence));
        }
      }
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
          for (const column of JSON_COLUMNS[table] ?? []) {
            if (typeof parsed[column] === "string") {
              parsed[column] = JSON.parse(parsed[column] as string);
            }
          }
          if (table === "part") {
            parsed.messageSequence = messageSequences.get(String(row.message_id)) ?? null;
          }
          const normalized = normalizeDeep(parsed, context) as JsonObject;
          // `normalizeDeep` 对 ID 键不看值，null 也会被折叠成 `<id>`；C2 需要保留
          // 「compact 的 turn_usage.user_message_id 是 null」这类事实。
          for (const [key, value] of Object.entries(parsed)) {
            if (value === null) normalized[key] = null;
          }
          return normalized;
        }),
      };
    }
    // `readDb` 已经覆盖 C1 的 5 张表，但它的规范化会把 null 的 ID 字段也折叠成 `<id>`，
    // 且 part 行丢失 message 关系。这里按 rowid 对齐原始行做一次补投影。
    const messageSequences = new Map<string, number>();
    for (const message of db.prepare("select id, sequence from message").all() as JsonObject[]) {
      messageSequences.set(String(message.id), Number(message.sequence));
    }
    for (const table of C2_TABLES) {
      const tableData = tables[table] as { rows?: JsonObject[] } | undefined;
      if (tableData?.rows === undefined) continue;
      const rawRows = db.prepare(`select * from ${table} order by rowid`).all() as JsonObject[];
      rawRows.forEach((raw, index) => {
        const row = tableData.rows?.[index];
        if (row === undefined) return;
        for (const [key, value] of Object.entries(raw)) {
          if (value === null) row[key] = null;
        }
        if (table === "part") {
          row.messageSequence = messageSequences.get(String(raw.message_id)) ?? null;
        }
      });
    }
  } finally {
    db.close();
  }
  return tables;
}

/** 只读采样某一列（用于 `session.time_compacting` 的进行中窗口）。 */
export function readSessionColumn(
  path: string,
  column: string,
): string | number | null | undefined {
  const db = new DatabaseSync(path, { readOnly: true });
  try {
    const row = db.prepare(`select ${column} as value from session limit 1`).get() as
      | { value: string | number | null }
      | undefined;
    return row?.value;
  } finally {
    db.close();
  }
}

/** 机器录制夹具：`expect` 全部来自本次运行，这里只负责落盘。 */
export async function writeC2Fixture(name: string, fixture: JsonObject): Promise<string> {
  await mkdir(C2_FIXTURE_DIR, { recursive: true });
  const target = fileURLToPath(new URL(`./fixtures/c2/${name}.json`, import.meta.url));
  await writeFile(target, `${JSON.stringify(fixture, null, 2)}\n`, "utf8");
  return target;
}

/** 投影模型请求体：保留可判定形状，折叠掉长提示词与工具 schema 正文。 */
export function projectModelRequest(
  body: JsonObject,
  normalize: (value: string) => string,
): JsonObject {
  const system = Array.isArray(body.system)
    ? (body.system as JsonObject[]).map((block) => {
        const text = typeof block.text === "string" ? normalize(block.text) : "";
        return {
          type: block.type,
          chars: text.length,
          head: text.slice(0, 200),
          memorySection: text.includes("# Memory"),
          memoryFileHeader: text.includes("(user's auto-memory, persists across conversations)"),
        };
      })
    : body.system;
  const messages = Array.isArray(body.messages)
    ? (body.messages as JsonObject[]).map((message) => {
        const content = message.content;
        const texts = Array.isArray(content)
          ? content
              .map((part) =>
                typeof (part as JsonObject).text === "string"
                  ? normalize((part as JsonObject).text as string)
                  : "",
              )
              .filter((text) => text.length > 0)
          : typeof content === "string"
            ? [normalize(content)]
            : [];
        const lastText = texts.at(-1) ?? "";
        return {
          role: message.role,
          contentTypes: Array.isArray(content)
            ? content.map((part) => (part as JsonObject).type ?? "text")
            : typeof content,
          textChars: texts.reduce((sum, text) => sum + text.length, 0),
          textHeads: texts.map((text) => text.slice(0, 300)),
          lastTextTail: lastText.slice(-700),
        };
      })
    : [];
  const tools = Array.isArray(body.tools)
    ? (body.tools as JsonObject[]).map((tool) => tool.name)
    : [];
  return {
    keys: Object.keys(body).sort(),
    model: body.model,
    max_tokens: body.max_tokens,
    stream: body.stream,
    thinking: body.thinking ?? null,
    system,
    messages,
    toolCount: tools.length,
    toolNames: tools,
  };
}

export type C2HarnessOptions = {
  prefix: string;
  script: MockStep[];
  seed?: Record<string, string>;
  runtimePreferencesReply?: Record<string, unknown>;
  /** 复用已有隔离根（memory 探针的「写入文件后重启」）；不传则 mkdtemp。 */
  dataRoot?: string;
  cwd?: string;
  /** 非流式/无工具请求的应答注入（C3 goal verifier 用；默认行为不变）。 */
  mockAuxiliaryResponse?: OracleRuntimeOptions["mockAuxiliaryResponse"];
  /** 纯观测钩子：mock provider 每收到一个请求体时回调（C3 记录重试到达时刻）。 */
  onMockRequest?: (path: string, body: JsonObject) => void;
  /** 纯观测钩子：每个原始 stdout 帧（C3 保留未二次规范化的 session/event）。 */
  onRawFrame?: (frame: JsonObject) => void;
  /** C4 生态面录制用：合并进 session/create 的额外参数（C1–C3 不传，路径不变）。 */
  sessionCreateParams?: Record<string, unknown>;
};

export type C2Harness = {
  session: OracleSession;
  captureState: Capture;
  context: PathContext;
  dataRoot: string;
  cwd: string;
  dbPath: string;
  /** 原始请求体（内存探针要检查完整注入文本）。 */
  rawBodies: JsonObject[];
  /** 原始请求体到达时的会话事件位置（用于证明「记忆抽取请求在 turn 终态之后」）。 */
  rawBodyMeta: JsonObject[];
  /** 已投影的请求体（compact/context 探针用）。 */
  bodies: JsonObject[];
  isolation(): JsonObject;
  sendAndWait(prompt: string): Promise<void>;
  stop(): Promise<void>;
};

/** 起一个隔离 oracle（dataRoot/cwd 都是 mkdtemp），并挂上录制钩子。 */
export async function startC2Harness(options: C2HarnessOptions): Promise<C2Harness> {
  const ownsDirs = options.dataRoot === undefined;
  const dataRoot =
    options.dataRoot ?? (await mkdtemp(join(tmpdir(), `mode-c2-${options.prefix}-`)));
  const cwd = options.cwd ?? (await mkdtemp(join(tmpdir(), `mode-c2-${options.prefix}-ws-`)));
  const context: PathContext = { dataRoot, cwd, tmpDir: tmpdir() };
  if (options.seed) await seedWorkspace(cwd, options.seed);
  const captureState = capture();
  const hooks = recordFrames(captureState, context);
  const rawBodies: JsonObject[] = [];
  const rawBodyMeta: JsonObject[] = [];
  const bodies: JsonObject[] = [];
  const session = await startOracleRuntime({
    dataRoot,
    cwd,
    script: options.script,
    runtimePreferencesReply: options.runtimePreferencesReply,
    mockAuxiliaryResponse: options.mockAuxiliaryResponse,
    sessionCreateParams: options.sessionCreateParams,
    onRawFrame: (frame) => {
      hooks.onFrame(frame);
      options.onRawFrame?.(frame);
    },
    onRawWrite: hooks.onWrite,
    onMockRequestBody: (path, body) => {
      const raw = body as JsonObject;
      rawBodies.push(raw);
      rawBodyMeta.push({
        index: rawBodies.length - 1,
        path,
        terminalCountAtArrival: terminalCount(captureState.events),
        lastEventTypeAtArrival: captureState.events.at(-1)?.type ?? null,
      });
      bodies.push({
        path,
        body: projectModelRequest(raw, (value) => normalizeC2String(value, context)),
      });
      options.onMockRequest?.(path, raw);
    },
  });
  const harness: C2Harness = {
    session,
    captureState,
    context,
    dataRoot,
    cwd,
    dbPath: join(dataRoot, ".mode", "cli", "db", "db.sqlite"),
    rawBodies,
    rawBodyMeta,
    bodies,
    isolation() {
      return {
        dataRootManifest: existsSync(join(dataRoot, ".mode", ".mode-root.json")),
        cliLayoutSentinel: existsSync(join(dataRoot, ".mode", "cli", "log")),
        invalidStdoutLines: session.invalidLines.length,
        mockRequestPaths: [...session.mockRequestPaths],
      };
    },
    async sendAndWait(prompt) {
      const expected = terminalCount(captureState.events) + 1;
      await session.send(prompt);
      await waitForTerminals(captureState, expected);
    },
    async stop() {
      await session.stop();
      if (ownsDirs && !process.argv.includes("--keep")) {
        await rm(dataRoot, { recursive: true, force: true });
        await rm(cwd, { recursive: true, force: true });
      }
    },
  };
  return harness;
}
