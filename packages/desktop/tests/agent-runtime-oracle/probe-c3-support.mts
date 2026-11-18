/* oxlint-disable eslint(max-lines) -- C3 投影/规范化/DB 读面共享同一套观察口径，拆散会复制规范化规则。 */
// C3 Task 0：goal / stream recovery / retry / background / subagents 探针的共享支撑。
//
// 与 `probe-c2-support.mts` 的分工：C2 负责隔离 oracle、帧录制、模型请求体录制；
// 这里只补 C3 需要的三件事：
//   1. 读 `session_target` / `session_task_link` 等 C3 表（同一套行投影规则）；
//   2. 把重试退避、目标计时这类**运行相关数值**折叠成稳定 token（夹具可复现）；
//   3. 等待某个具体事件/终态（不用固定 sleep 当证据）。
//
// clean-room：本文件只运行 oracle 并读它写出的帧/行/文件，不读实现体。
import { DatabaseSync } from "node:sqlite";
import { existsSync } from "node:fs";
import { mkdir, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

import type { JsonObject, PathContext } from "./session-lifecycle-fixtures.mjs";
import { normalizeDeep } from "./session-lifecycle-fixtures.mjs";
import {
  delay,
  normalizeC2String,
  startC2Harness,
  stripErrorStack,
  terminalCount,
  waitForTerminals,
  type C2Harness,
  type C2HarnessOptions,
} from "./probe-c2-support.mjs";

export const C3_FIXTURE_DIR = fileURLToPath(new URL("./fixtures/c3/", import.meta.url));

/** C3 需要读的表；顺序固定，夹具按 rowid 记录。 */
export const C3_TABLES = [
  "session",
  "message",
  "part",
  "session_input",
  "session_entry",
  "session_target",
  "session_task_link",
  "tool_usage",
  "turn_usage",
  "model_usage",
] as const;

const C3_JSON_COLUMNS: Record<string, readonly string[]> = {
  session: ["permission"],
  message: ["data"],
  part: ["data"],
  session_input: ["payload"],
  session_entry: ["data"],
  model_usage: ["raw_usage_json", "provider_metadata_json"],
};

/**
 * 夹具里必须折叠的运行相关数值：
 * - 重试退避带抖动，逐次运行不同；0 与非 0 语义不同，保留 0；
 * - `timeUsedSeconds` 是墙钟累计；
 * - `tokensPerSecond` 是耗时派生的速率；
 * - `pid` 是机器进程号。
 */
const C3_PRESERVE_NUMERIC_KEYS = new Set([
  "delayMs",
  "retryDelayMs",
  "timeUsedSeconds",
  "time_used_seconds",
  "tokensPerSecond",
  "prefillTokensPerSecond",
  "pid",
]);

const TARGET_ID_RE = /\btarget_[a-z0-9]+_(?:<id>|[0-9a-f]{8}-[0-9a-f-]{27,})\b/giu;

/** 字符串里的目标 id / 用量文案折叠；其余保持观察原文。 */
export function normalizeC3String(value: string, context: PathContext): string {
  return (
    normalizeC2String(value, context)
      .replace(TARGET_ID_RE, "<target-id>")
      // verificationId 是截短 UUID（8-4-2），完整 UUID 正则不会命中。
      .replace(/\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{2,12}\b/giu, "<id>")
      .replace(/Usage: \d+ tokens \/ (?:none|\d+)/gu, "Usage: <tokens> tokens / <budget>")
      .replace(/Time: \d+ seconds/gu, "Time: <time>")
      .replace(/Time spent pursuing goal: \d+ seconds/gu, "Time spent pursuing goal: <time>")
      .replace(/Tokens used: \d+/gu, "Tokens used: <tokens>")
      .replace(/Tokens remaining: (?:\d+|unbounded)/gu, "Tokens remaining: <tokens>")
      .replace(/Token budget: \d+/gu, "Token budget: <tokens>")
      // 工具结果/后台通知正文里的耗时是墙上时间。
      .replace(/duration_ms: \d+/gu, "duration_ms: <time>")
      .replace(/<duration_ms>\d+<\/duration_ms>/gu, "<duration_ms><time></duration_ms>")
  );
}

/** 对 `normalizeDeep` 的输出再做 C3 稳定化（目标 id、退避、速率、pid）。 */
export function normalizeC3Deep(value: unknown, context: PathContext): unknown {
  const base = normalizeDeep(value, context, {
    preserveNumericKeys: C3_PRESERVE_NUMERIC_KEYS,
  });
  const walk = (inner: unknown, key?: string): unknown => {
    if (key === "target_id" || key === "targetId" || key === "targetID") {
      return inner === null || inner === undefined ? inner : "<target-id>";
    }
    if (key === "pid") return inner === null || inner === undefined ? inner : "<pid>";
    if (key === "timeUsedSeconds" || key === "time_used_seconds") {
      return inner === null || inner === undefined ? inner : "<time>";
    }
    if (key === "delayMs" || key === "retryDelayMs") {
      return typeof inner === "number" && inner === 0 ? 0 : "<retry-delay>";
    }
    if (key === "tokensPerSecond" || key === "prefillTokensPerSecond") {
      return inner === null || inner === undefined ? inner : "<rate>";
    }
    if (typeof inner === "string") return normalizeC3String(inner, context);
    if (Array.isArray(inner)) return inner.map((item) => walk(item));
    if (inner === null || typeof inner !== "object") return inner;
    return Object.fromEntries(
      Object.entries(inner as JsonObject).map(([childKey, child]) => [
        childKey,
        walk(child, childKey),
      ]),
    );
  };
  return walk(base);
}

/** 读全部 C3 表；JSON 列解析、null ID 保留、part 行补 messageSequence。 */
export function readC3Db(path: string, context: PathContext): JsonObject {
  const db = new DatabaseSync(path, { readOnly: true });
  try {
    const tables: JsonObject = {};
    const messageSequences = new Map<string, number>();
    if (C3_TABLES.includes("message")) {
      for (const message of db.prepare("select id, sequence from message").all() as JsonObject[]) {
        messageSequences.set(String(message.id), Number(message.sequence));
      }
    }
    for (const table of C3_TABLES) {
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
          for (const column of C3_JSON_COLUMNS[table] ?? []) {
            if (typeof parsed[column] === "string") {
              parsed[column] = JSON.parse(parsed[column] as string);
            }
          }
          if (table === "part") {
            parsed.messageSequence = messageSequences.get(String(row.message_id)) ?? null;
          }
          const normalized = normalizeC3Deep(parsed, context) as JsonObject;
          // normalizeDeep 会把 null 的 ID 键也折叠；C3 需要保留「显式为 null」。
          for (const [key, value] of Object.entries(parsed)) {
            if (value === null) normalized[key] = null;
          }
          if (table === "part") {
            normalized.messageSequence = messageSequences.get(String(row.message_id)) ?? null;
          }
          return normalized;
        }),
      };
    }
    return tables;
  } finally {
    db.close();
  }
}

export type C3RequestRecord = { at: number; path: string; body: JsonObject };

export type C3Harness = C2Harness & {
  /** mock provider 收到的请求（含到达时刻，用于观察重试间隔）。 */
  requests: C3RequestRecord[];
  /** 读 C3 全表（每次调用都读当前磁盘状态）。 */
  readTables(): JsonObject;
  /** `session/read` 的规范化投影。 */
  readSession(): Promise<JsonObject>;
  /** `session/debug` 的规范化投影。 */
  readDebug(): Promise<JsonObject>;
  /** 从 `start` 下标开始等待满足条件的事件。 */
  waitForEvent(
    start: number,
    predicate: (event: JsonObject) => boolean,
    what: string,
    timeoutMs?: number,
  ): Promise<JsonObject>;
  /** 从 `start` 下标开始的全部事件（规范化已在 capture 里完成）。 */
  eventsSince(start: number): JsonObject[];
  /** 当前事件条数（用于切片边界）。 */
  eventCount(): number;
};

export type C3HarnessOptions = C2HarnessOptions;

/** 起一个隔离 oracle，并在 C2 harness 之上补 C3 的读面。 */
export async function startC3Harness(options: C3HarnessOptions): Promise<C3Harness> {
  const requests: C3RequestRecord[] = [];
  const rawEvents: JsonObject[] = [];
  const harness = await startC2Harness({
    ...options,
    onMockRequest: (path, body) => {
      requests.push({ at: Date.now(), path, body });
      options.onMockRequest?.(path, body);
    },
    onRawFrame: (frame) => {
      if (
        frame.method === "session/event" &&
        frame.params !== null &&
        typeof frame.params === "object"
      ) {
        rawEvents.push(frame.params as JsonObject);
      }
      options.onRawFrame?.(frame);
    },
  });
  const c3: C3Harness = {
    ...harness,
    requests,
    readTables() {
      return readC3Db(harness.dbPath, harness.context);
    },
    async readSession() {
      const result = await harness.session.request("session/read", {
        sessionId: harness.session.sessionId,
      });
      return normalizeC3Deep(result, harness.context) as JsonObject;
    },
    async readDebug() {
      const result = await harness.session.request("session/debug", {
        sessionId: harness.session.sessionId,
      });
      return normalizeC3Deep(result, harness.context) as JsonObject;
    },
    eventCount() {
      return rawEvents.length;
    },
    eventsSince(start) {
      return rawEvents
        .slice(start)
        .map((event) => normalizeC3Deep(event, harness.context) as JsonObject);
    },
    async waitForEvent(start, predicate, what, timeoutMs = 60_000) {
      const deadline = Date.now() + timeoutMs;
      while (true) {
        const found = rawEvents
          .slice(start)
          .map((event) => normalizeC3Deep(event, harness.context) as JsonObject)
          .find(predicate);
        if (found !== undefined) return found;
        if (Date.now() > deadline) {
          throw new Error(`oracle 在 ${timeoutMs}ms 内没有观察到 ${what}`);
        }
        await delay(20);
      }
    },
  };
  return c3;
}

/** 解析 driver 抛出的 JSON-RPC 错误帧；解析失败时保留原文。 */
export function errorRecord(error: unknown): JsonObject {
  const text = String(error);
  const match = /失败：(\{[\s\S]*\})$/u.exec(text);
  if (match?.[1] !== undefined) {
    try {
      return { frame: JSON.parse(match[1]) as JsonObject };
    } catch {
      // 落到下面的原文分支。
    }
  }
  return { message: text };
}

/** 错误帧的规范化投影（C3 的 -32009/-32010/-32602 形状会进夹具）。 */
export function normalizeError(error: unknown, context: PathContext): JsonObject {
  return normalizeC3Deep(stripErrorStack(errorRecord(error)), context) as JsonObject;
}

export type GoalCall = {
  response?: string;
  startedTurn?: boolean;
  snapshot?: JsonObject;
  [key: string]: unknown;
};

/** 调用 `session/goal`；成功/失败都规范化，失败不抛出。 */
export async function callGoal(
  harness: C3Harness,
  params: Record<string, unknown>,
): Promise<{ ok: true; result: GoalCall } | { ok: false; error: JsonObject }> {
  try {
    const result = (await harness.session.request("session/goal", {
      sessionId: harness.session.sessionId,
      ...params,
    })) as GoalCall;
    return {
      ok: true,
      result: normalizeC3Deep(result, harness.context) as GoalCall,
    };
  } catch (error) {
    return { ok: false, error: normalizeError(error, harness.context) };
  }
}

/** 只取 snapshot 里 goal 相关的投影，避免夹具重复整份快照。 */
export function projectGoalSnapshot(result: GoalCall | undefined): JsonObject | null {
  if (result?.snapshot === undefined) return null;
  const snapshot = result.snapshot as JsonObject;
  const projection = (snapshot.projection ?? {}) as JsonObject;
  const session = (snapshot.session ?? {}) as JsonObject;
  const runtime = (snapshot.runtime ?? {}) as JsonObject;
  return {
    target: projection.target ?? null,
    sessionTarget: session.target ?? null,
    projectionStatus: projection.status ?? null,
    turnCount: projection.turnCount ?? null,
    currentTurnId: projection.currentTurnId ?? null,
    sessionStatus: session.status ?? null,
    eventSeq: runtime.eventSeq ?? null,
    stateRevision: runtime.stateRevision ?? null,
    goalVerifications: runtime.goalVerifications ?? null,
    goalVerificationTimeline: runtime.goalVerificationTimeline ?? null,
  };
}

/** 对指定会话调用 `session/read`（冷重启后读旧会话时用）。 */
export async function readSessionById(harness: C3Harness, sessionId: string): Promise<JsonObject> {
  const result = await harness.session.request("session/read", { sessionId });
  return normalizeC3Deep(result, harness.context) as JsonObject;
}

/** `session/read` 的 goal 投影（与 snapshot 同源，但读面会带上完整快照键）。 */
export function projectGoalRead(read: JsonObject): JsonObject {
  const projection = (read.projection ?? {}) as JsonObject;
  const session = (read.session ?? {}) as JsonObject;
  const runtime = (read.runtime ?? {}) as JsonObject;
  return {
    target: session.target ?? null,
    projectionTarget: projection.target ?? null,
    sessionStatus: session.status ?? null,
    projectionStatus: projection.status ?? null,
    turnCount: projection.turnCount ?? null,
    eventSeq: runtime.eventSeq ?? null,
    stateRevision: runtime.stateRevision ?? null,
    goalVerifications: runtime.goalVerifications ?? null,
    goalVerificationTimeline: runtime.goalVerificationTimeline ?? null,
  };
}

/** 事件投影：只保留 C3 关心的事件类型，避免夹具里混入启动/遥测噪声。 */
export function projectEvents(
  events: readonly JsonObject[],
  keep: (event: JsonObject) => boolean = () => true,
): JsonObject[] {
  return events.filter(keep).map((event) => {
    const payload = stripErrorStack(event.payload);
    return { ...event, payload };
  });
}

/** 事件类型序列 + 每个终态的 resultType，供契约文档快速引用。 */
export function eventSummary(events: readonly JsonObject[]): JsonObject {
  return {
    types: events.map((event) => event.type),
    terminals: events
      .filter((event) => event.type === "turn.completed" || event.type === "turn.failed")
      .map((event) => ({
        type: event.type,
        resultType: (event.payload as JsonObject | undefined)?.resultType ?? null,
      })),
    turnStarts: events
      .filter((event) => event.type === "turn.started")
      .map((event) => {
        const payload = (event.payload ?? {}) as JsonObject;
        return {
          turnNumber: payload.turnNumber ?? null,
          executionKind: payload.executionKind ?? null,
          inputSource: payload.inputSource ?? null,
          input: payload.input ?? null,
          targetId: payload.targetId ?? null,
        };
      }),
  };
}

/** 从 session 事件里抽 text_delta，检查重试是否重复。 */
export function textDeltas(events: readonly JsonObject[]): string[] {
  return events
    .filter((event) => event.type === "model.streaming")
    .map((event) => (event.payload ?? {}) as JsonObject)
    .filter((payload) => payload.kind === "text_delta" && typeof payload.delta === "string")
    .map((payload) => payload.delta as string);
}

/** 只读 DB 投影：按 C3 关注面裁剪行，避免整个库进夹具。 */
export function projectGoalDb(tables: JsonObject): JsonObject {
  const tableRows = (table: string): JsonObject[] =>
    (((tables[table] as JsonObject | undefined)?.rows as JsonObject[] | undefined) ?? []).slice();
  return {
    session_target: {
      rowCount: (tables.session_target as JsonObject | undefined)?.rowCount ?? null,
      rows: tableRows("session_target"),
    },
    session_task_link: {
      rowCount: (tables.session_task_link as JsonObject | undefined)?.rowCount ?? null,
      rows: tableRows("session_task_link"),
    },
    session_entry: {
      rowCount: (tables.session_entry as JsonObject | undefined)?.rowCount ?? null,
      rows: tableRows("session_entry").filter((row) =>
        String(row.type).startsWith("target_completion_verification"),
      ),
    },
    turn_usage: {
      rowCount: (tables.turn_usage as JsonObject | undefined)?.rowCount ?? null,
      rows: tableRows("turn_usage"),
    },
    model_usage: {
      rowCount: (tables.model_usage as JsonObject | undefined)?.rowCount ?? null,
      rows: tableRows("model_usage").map((row) => ({
        query_source: row.query_source,
        status: row.status,
        attempt_index: row.attempt_index,
        retry_count: row.retry_count,
        retryable: row.retryable,
        finish_reason: row.finish_reason,
        input_tokens: row.input_tokens,
        output_tokens: row.output_tokens,
        computed_total_tokens: row.computed_total_tokens,
        error_type: row.error_type,
        error_code: row.error_code,
      })),
    },
    messageSummary: tableRows("message").map((row) => {
      const data = (row.data ?? {}) as JsonObject;
      return {
        sequence: row.sequence ?? null,
        role: data.role ?? null,
        finish: data.finish ?? null,
        semanticsKind: (data.semantics as JsonObject | undefined)?.kind ?? null,
        source: data.source ?? null,
        parentID: data.parentID ?? null,
      };
    }),
  };
}

/** 机器录制夹具：`expect` 全部来自本次运行，这里只负责落盘。 */
export async function writeC3Fixture(name: string, fixture: JsonObject): Promise<string> {
  await mkdir(C3_FIXTURE_DIR, { recursive: true });
  const target = fileURLToPath(new URL(`./fixtures/c3/${name}.json`, import.meta.url));
  await writeFile(target, `${JSON.stringify(fixture, null, 2)}\n`, "utf8");
  return target;
}

/** 隔离断言；与 C1/C2 同一口径。 */
export function isolationRecord(harness: C3Harness): JsonObject {
  return {
    ...harness.isolation(),
    cliLayoutSentinel: existsSync(`${harness.dataRoot}/.mode/cli/log`),
  };
}

/** 等第 `expected` 个终态；保留 C2 的 500ms 收尾窗。 */
export async function waitForTerminalCount(
  harness: C3Harness,
  expected: number,
  timeoutMs = 180_000,
): Promise<void> {
  await waitForTerminals(harness.captureState, expected, timeoutMs);
}

export { delay, terminalCount };
