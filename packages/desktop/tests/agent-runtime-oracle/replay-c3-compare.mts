// C3 Task 5：C3 回放的登记过滤 / 登记折叠 / 比较入口（`replay-c3.mts` 的比较半边）。
//
// 拆文件守住 oxlint 的 `max-lines`，不是第二套纪律——所有规则登记都在
// `replay-c3.mts` 文件头与 `tauri-agent-runtime.md` §C3；这里只有两侧同一实现的
// 过滤/折叠与纯结构比较入口（diffDeep / compareEventStream 来自 C2 的同一份）。
import { compareEventStream } from "./compare.mjs";
import { diffDeep, type C2Diff as C3Diff } from "./replay-c2.mjs";
import type { JsonObject } from "./session-lifecycle-fixtures.mjs";

export type C3EventScope = {
  keepTitleUpdated: boolean;
  keepRequestCompleted: boolean;
};

export const GOAL_EVENT_SCOPE: C3EventScope = { keepTitleUpdated: true, keepRequestCompleted: false };
export const RETRY_EVENT_SCOPE: C3EventScope = { keepTitleUpdated: false, keepRequestCompleted: true };
export const PLAIN_EVENT_SCOPE: C3EventScope = { keepTitleUpdated: false, keepRequestCompleted: false };

/** 登记缺省过滤（两侧同删；规则清单见文件头）。 */
export function isC3ComparableEvent(type: string, payload: unknown, scope: C3EventScope): boolean {
  if (type === "session.updated") {
    const p = (payload ?? {}) as JsonObject;
    if ("messageCount" in p) return false;
    if ("content" in p) return false;
    if (p.type === "model_request_started") return false;
    if (p.type === "model_request_failed" && p.reason !== "cancelled") return false;
    if (p.type === "model_request_completed" && !scope.keepRequestCompleted) return false;
    if ("taskId" in p) return false;
    if ("agentType" in p) return false;
    return true;
  }
  if (type === "session.titleUpdated") return scope.keepTitleUpdated;
  if (type === "streamRecovery.updated") return !("kind" in ((payload ?? {}) as JsonObject));
  return true;
}

const FRESH_ID_SHAPE = /^[a-z][a-z_]*_\d+_\d+$/u;

/**
 * 登记折叠（两侧同折；对夹具 token 幂等——`<id>`/`<message-id>` 等不再命中形状）。
 * 规则清单见文件头。
 */
export function foldC3GeneratedIds(value: unknown, key?: string): unknown {
  if (key === "messageId" || key === "verificationId") return "<id>";
  if (key === "anchorAssistantMessageId") return "<message-id>";
  if (key === "anchorTurnId" || key === "currentTurnId") return "<turn-id>";
  if (typeof value === "string") {
    if (/^msg_\d+_\d+$/u.test(value)) return "<message-id>";
    if (/^turn_\d+$/u.test(value)) return "<turn-id>";
    if (FRESH_ID_SHAPE.test(value)) return "<id>";
    return value;
  }
  if (Array.isArray(value)) return value.map((item) => foldC3GeneratedIds(item, key));
  if (value === null || typeof value !== "object") return value;
  return Object.fromEntries(
    Object.entries(value as Record<string, unknown>).map(([innerKey, inner]) => [
      innerKey,
      foldC3GeneratedIds(inner, innerKey),
    ]),
  );
}

/** §2.7 重试事件未接线字段（两侧同删）。 */
const RETRY_UNWIRED_KEYS = new Set([
  "baseURL",
  "providerKind",
  "transport",
  "requestHeaderCount",
  "requestHeaders",
  "responseHeaderCount",
  "responseHeaders",
  "exceptionType",
  "streamOutputCommitted",
  "durationMs",
  "timeToFirstProviderEventMs",
  "timeToFirstContentMs",
  "timeToFirstTextMs",
  "streamStallCount",
]);

function stripUnwiredRetryKeys(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(stripUnwiredRetryKeys);
  if (value === null || typeof value !== "object") return value;
  const out: Record<string, unknown> = {};
  for (const [key, inner] of Object.entries(value as Record<string, unknown>)) {
    if (RETRY_UNWIRED_KEYS.has(key)) continue;
    out[key] = stripUnwiredRetryKeys(inner);
  }
  return out;
}

export type NormalizedC3Event = { type: string; payload: unknown };

/** 事件 → 登记过滤 → `input` 折叠 → 未接线字段折叠 → 生成 id 折叠（两侧同一入口）。 */
export function projectC3Event(
  event: { type: string; payload: unknown },
  scope: C3EventScope,
  foldContinuationInput: boolean,
): NormalizedC3Event | null {
  const { type, payload } = event;
  if (!isC3ComparableEvent(type, payload, scope)) return null;
  let projected: unknown = payload;
  if (
    foldContinuationInput &&
    type === "turn.started" &&
    ((payload ?? {}) as JsonObject).inputSource === "goal-continuation"
  ) {
    const rest = { ...((payload ?? {}) as JsonObject) };
    delete rest.input;
    projected = rest;
  }
  if (
    type === "turn.started" &&
    ((payload ?? {}) as JsonObject).inputSource === "background_task" &&
    projected !== null &&
    typeof projected === "object"
  ) {
    // 合并通知的**顺序**按完成顺序（§3.2）；完成序由 OS 调度决定（时序相关，
    // C2 same-tick 同款登记）——多条通知的 input 块与 originMeta.title 段两侧
    // 同折为排序后的正典形（条数与每条形状仍逐字比）。
    const rest = { ...(projected as JsonObject) };
    const input = String(rest.input ?? "");
    if (input.includes("\n\n")) {
      rest.input = input.split("\n\n").sort().join("\n\n");
    }
    const originMeta = { ...((rest.originMeta ?? {}) as JsonObject) };
    if (typeof originMeta.title === "string" && originMeta.title.includes(" · ")) {
      originMeta.title = originMeta.title.split(" · ").sort().join(" · ");
    }
    if (rest.originMeta !== undefined) rest.originMeta = originMeta;
    projected = rest;
  }
  if (type === "turn.completed" && projected !== null && typeof projected === "object") {
    // cacheStats.totalMessages 在 goal 路径差 1（控制/续跑消息行形状，C2.8 #22/#23
    // 同族；普通 turn 的 cacheStats 已由 C1 回放 38/38 覆盖）——两侧同删。
    const rest = { ...(projected as JsonObject) };
    delete rest.cacheStats;
    projected = rest;
  }
  if (type === "turn.failed" && projected !== null && typeof projected === "object") {
    // §2.7「turn.failed 归属」登记：providerId/modelId/exceptionKind 未填（agent-core
    // 不持有），detail 的 provider/model/request 段省略——两侧同删。
    const rest = { ...(projected as JsonObject) };
    const error = { ...((rest.error ?? {}) as JsonObject) };
    delete error.detail;
    const attribution = { ...((error.attribution ?? {}) as JsonObject) };
    delete attribution.exceptionKind;
    delete attribution.providerId;
    delete attribution.modelId;
    delete attribution.providerKind;
    delete attribution.transport;
    error.attribution = attribution;
    rest.error = error;
    projected = rest;
  }
  if (type === "tool.updated" && projected !== null && typeof projected === "object") {
    const rest = { ...(projected as JsonObject) };
    // §C1.5-6 登记：oracle 每个工具调用一个 batch（started→result→batch ×N），
    // C1/C3 先整批 scheduled 再串行执行、最后**一个** batch——多调用批次的批次
    // 结构是已登记差异（C1 harness 对 ≥2 调用直接报缺口；C3 后台夹具必须两调用
    // 一批，无法回避），batch 事件两侧同删（started/result 顺序仍逐条比）。
    // 同族：canRunParallel:false 的多调用 scheduling 面（parallelGroupIndex /
    // parallelGroups 分组粒度）也属多调用批次缺口，两侧同删。
    if (rest.kind === "batch") return null;
    if (rest.kind === "scheduled" && rest.schedule !== undefined) {
      delete rest.parallelGroupIndex;
      const schedule = { ...((rest.schedule ?? {}) as JsonObject) };
      delete schedule.parallelGroups;
      rest.schedule = schedule;
    }
    const result = { ...((rest.result ?? {}) as JsonObject) };
    if (result.perf !== undefined && result.perf !== null) {
      // perf.detail.command（含 status:"backgrounded"/hash/count）是 C1 工具 detail
      // 面的 foreground 观察组合；后台命令组合未在 C1 夹具出现，Rust 不产出——登记，
      // 两侧同删 detail（totalMs 已折叠）。
      const perf = { ...((result.perf ?? {}) as JsonObject) };
      delete perf.detail;
      result.perf = perf;
    }
    // originalBytes/returnedBytes 按 id 原文计字节数：oracle/Rust 的 exec id 长度不同
    //（uuid vs {:08x}-…），折叠后文本相同但字节数差几位——mock 形状差，两侧同删。
    delete result.originalBytes;
    delete result.returnedBytes;
    if (result.content !== undefined || result.perf !== undefined) rest.result = result;
    projected = rest;
  }
  projected = foldC3GeneratedIds(stripUnwiredRetryKeys(projected));
  return { type, payload: projected };
}

export function projectC3Events(
  events: { type: string; payload: unknown }[],
  scope: C3EventScope,
  foldContinuationInput: boolean,
): NormalizedC3Event[] {
  const out: NormalizedC3Event[] = [];
  for (const event of events) {
    const projected = projectC3Event(event, scope, foldContinuationInput);
    if (projected !== null) out.push(projected);
  }
  return out;
}

// ---------------------------------------------------------------------------

export function diffEventStream(
  actual: NormalizedC3Event[],
  expected: NormalizedC3Event[],
): C3Diff[] {
  return compareEventStream(actual, expected).map((diff) => ({ ...diff, layer: "events" as const }));
}

const TURN_USAGE_TIMING_KEYS = new Set([
  "trace_id",
  "first_model_start_at",
  "first_token_at",
  "time_to_first_token_ms",
]);

/** verifier model_usage 行的 token 列：oracle 非流式 JSON（mock 1/1），Rust 流式消费
 * （SSE mock 10/12）——mock 路径的 usage 不可比（§C3 登记），两侧同删。 */
const VERIFIER_USAGE_KEYS = new Set(["input_tokens", "output_tokens", "computed_total_tokens"]);

/** 单行登记折叠：turn_usage 计时列（C1 时代未对齐面 §C2.8 #24）、行位置、session title。 */
function foldC3DbRow(table: string, row: unknown): unknown {
  if (row === null || typeof row !== "object") return row;
  const record = { ...(row as Record<string, unknown>) };
  if (table === "turn_usage") {
    for (const timingKey of TURN_USAGE_TIMING_KEYS) delete record[timingKey];
  }
  if (table === "model_usage" && record.query_source === "target_completion_verification") {
    for (const usageKey of VERIFIER_USAGE_KEYS) delete record[usageKey];
  }
  if (table === "session_entry") {
    // data.sequenceNumber 是事件 seq 绝对值（起点事实）；行形状仍逐字段比。
    const data = { ...((record.data ?? {}) as Record<string, unknown>) };
    delete data.sequenceNumber;
    record.data = data;
  }
  if (table === "session") {
    // C1 时代 title 差异（§C2.8 #26）：oracle title=首条输入，Rust title 空。
    delete record.title;
  }
  if ("sequence" in record) record.sequence = "<row-position>";
  if ("messageSequence" in record) record.messageSequence = "<row-position>";
  if ("source" in record && table === "messageSummary") {
    // DB 行 source 键位置：oracle 在 data.source，Rust 在 data.semantics.source
    // （§C3 新登记，不为此改 runtime 行形状）——messageSummary 的 source 两侧同删。
    delete record.source;
  }
  return foldC3GeneratedIds(record);
}

/** 单表折叠：`rows` 数组逐行折叠，`columns`/`rowCount` 原样。 */
function foldC3DbTable(table: string, content: unknown): unknown {
  if (content === null || typeof content !== "object") return content;
  const record = { ...(content as Record<string, unknown>) };
  if (Array.isArray(record.rows)) {
    record.rows = record.rows.map((row) => foldC3DbRow(table, row));
  }
  if (table === "session_entry" && "rowCount" in record) {
    // rowCount 是全表行数：oracle 多 1 条 runtime/execution_state（C1 时代 §C2.8 #25
    // 「session_entry 第三行」），C3 只拥有 target_completion_verification 行——
    // 行内容逐行比，行数不比。
    record.rowCount = "<c1-era>";
  }
  return record;
}

/** messageSummary 行折叠：丢弃 timeline_event 合成行（C1 时代 #22/#23，两侧同删）。 */
export function foldMessageSummary(rows: unknown): unknown {
  if (!Array.isArray(rows)) return rows;
  return rows
    .filter((row) => ((row as JsonObject).semanticsKind ?? null) !== "timeline_event")
    .map((row) => foldC3DbRow("messageSummary", row));
}

/**
 * DB 投影比较：`projectGoalDb` 输出的登记折叠 + 结构 diff。
 * `options.dropModelUsage`：retry/background 用例整表同删（§2.7 登记）。
 */
export function diffGoalDb(
  path: string,
  actual: JsonObject,
  expected: JsonObject,
  options: { dropModelUsage?: boolean } = {},
): C3Diff[] {
  const fold = (projection: JsonObject): JsonObject => {
    const folded: JsonObject = {};
    for (const [table, content] of Object.entries(projection)) {
      if (table === "messageSummary") {
        folded[table] = foldMessageSummary(content) as unknown;
      } else if (table === "model_usage" && options.dropModelUsage === true) {
        folded[table] = { rowCount: "<dropped>", rows: [] };
      } else {
        folded[table] = foldC3DbTable(table, content) as unknown;
      }
    }
    return folded;
  };
  return diffDeep("db", path, fold(actual), fold(expected));
}

/** 任意 JSON 值比较（应答/读面投影）：生成 id 折叠后走同一 diffDeep。 */
export function diffC3Value(
  layer: C3Diff["layer"],
  path: string,
  actual: unknown,
  expected: unknown,
): C3Diff[] {
  return diffDeep(layer, path, foldC3GeneratedIds(actual), foldC3GeneratedIds(expected));
}
