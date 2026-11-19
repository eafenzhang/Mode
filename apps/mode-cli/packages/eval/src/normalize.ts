// LT-4 规范化（spec §6 的 v1 词表）。
//
// 规则来源是 oracle 的规范化规范（`docs/specs/tauri-agent-runtime.md` §3 的白名单：
// 无序组、id、时间、路径、错误栈、命令指纹、传输头、环境计数）。本实现按 spec §6
// 落成 v1 子集，并由一致性测试钉住占位符词表；比较器不重复另一套规则。
import type { EvalEvent, ToolCall } from "./types.js";

export type NormalizeContext = {
  /** case 的 workspace（写种子文件、agent-server 的工作目录）。 */
  cwd: string;
  /** 隔离数据根（`MODE_DATA_BASE_DIR`）。 */
  dataRoot: string;
  /** 系统临时目录；隔离根与 workspace 都在它下面，替换顺序必须从长到短。 */
  tmpDir: string;
};

export type ProjectedEvent = { type: string; payload: unknown };

/** 每次运行必然不同的 id 键（与 oracle §3 同表，避免把 providerId/modelId 这类事实键一起抹掉）。 */
const VOLATILE_ID_KEYS: ReadonlySet<string> = new Set([
  "id",
  "eventId",
  "sessionId",
  "childSessionId",
  "turnId",
  "messageId",
  "messageID",
  "assistantMessageId",
  "parentMessageId",
  "anchorId",
  "anchorMessageId",
  "summaryMessageId",
  "targetMessageId",
  "toolMessageId",
  "toolCallId",
  "callId",
  "requestId",
  "parentRequestId",
  "inputId",
  "queryId",
  "traceId",
  "parentId",
  "spanId",
  "parentSpanId",
  "attemptId",
  "databaseId",
  "agentId",
  "targetId",
  "verificationId",
  "operationId",
  "logicalCallId",
  "resultPartId",
  "checkpointId",
  "boundaryId",
  "submissionId",
  "mcpId",
  "mcpInstanceId",
  "foregroundExecutionId",
]);

const VOLATILE_ID_LIST_KEYS: ReadonlySet<string> = new Set([
  "toolCallIds",
  "pendingRequestIds",
  "requestIds",
  "committedToolCallIds",
]);

/** id 容器键：容器结构是行为，只有最内层的字符串元素是 id。 */
const ID_CONTAINER_KEYS: ReadonlySet<string> = new Set(["parallelGroups", "executionOrder"]);

/** 时间键：ISO 字符串与数值耗时都折叠成 `<time>`。 */
const TIME_KEYS = /^(timestamp|duration|date|elapsedMs|.*At|.*Ms)$/;

/** 错误栈是构建产物，不是运行时行为：整体删除。 */
const DROPPED_STACK_KEY = "stack";

/** 命令内容指纹（同一对象同时带 category/status/hash）不是行为：删除 `hash`。 */
const TRANSPORT_HEADER_KEYS: ReadonlySet<string> = new Set(["requestHeaders", "responseHeaders"]);

/** 随环境变化的计数（装了哪些 MCP/插件/技能不是被测量行为）。 */
const ENVIRONMENT_COUNT_KEYS: ReadonlySet<string> = new Set(["toolCount", "chars"]);

const UUID_PATTERN = /[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}/g;
const LOOPBACK_URL_PATTERN = /\b(?:https?|wss?):\/\/(?:127\.0\.0\.1|localhost|\[::1\]):\d+/g;

/** 状态推送：并发发出的相对顺序不是行为。 */
const UNORDERED_EVENT_TYPES: ReadonlySet<string> = new Set(["session.updated"]);

/** 稳定序列化：键递归排序，用于比较与无序组排序键。 */
export function stableStringify(value: unknown): string {
  return JSON.stringify(sortValue(value));
}

function sortValue(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(sortValue);
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value as Record<string, unknown>)
        .sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0))
        .map(([key, inner]) => [key, sortValue(inner)]),
    );
  }
  return value;
}

function jsonEscaped(value: string): string {
  return JSON.stringify(value).slice(1, -1);
}

/** 长路径在前：短前缀不会把长路径切坏。 */
function pathTokens(context: NormalizeContext): Array<[string, string]> {
  return [
    [context.dataRoot, "<data-root>"],
    [context.cwd, "<cwd>"],
    [context.tmpDir, "<tmp>"],
  ];
}

function normalizeString(value: string, context: NormalizeContext): string {
  let result = canonicalizeJsonObjectText(value);
  for (const [raw, token] of pathTokens(context)) {
    if (raw.length === 0) continue;
    result = result.replaceAll(raw, token);
    const escaped = jsonEscaped(raw);
    if (escaped !== raw) {
      result = result.replaceAll(escaped, token);
    }
  }
  result = result.replace(LOOPBACK_URL_PATTERN, "<loopback-url>");
  // shell 诊断前缀：启动方式（argv[0]/包装行数）的差异不是工具行为。
  result = result.replace(/(^|\n)[^\n]*?bash(?:\.exe)?: line \d+:/g, "$1<shell>: line <n>:");
  // 只认 uuid 形状，不做「见到 xxx_yyy 就当 id」的按形状匹配：会误伤工具输出正文。
  result = result.replace(UUID_PATTERN, "<id>");
  return result;
}

/** 字符串整体是一个 JSON 对象时按对象键稳定排序后重新序列化；数组顺序可能是行为，不碰。 */
function canonicalizeJsonObjectText(value: string): string {
  const trimmed = value.trim();
  if (!trimmed.startsWith("{")) return value;
  try {
    const parsed: unknown = JSON.parse(trimmed);
    if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) return value;
    return JSON.stringify(sortValue(parsed));
  } catch {
    return value;
  }
}

function isCommandFingerprintObject(value: Record<string, unknown>): boolean {
  return "category" in value && "status" in value && "hash" in value;
}

function normalizeIdContainer(value: unknown): unknown {
  if (typeof value === "string") return "<id>";
  if (Array.isArray(value)) return value.map((item) => normalizeIdContainer(item));
  return value;
}

export function normalizeValue(value: unknown, context: NormalizeContext): unknown {
  if (typeof value === "string") return normalizeString(value, context);
  if (Array.isArray(value)) {
    // 不排序：数组顺序常常是行为，顺序差异只能由无序组显式豁免。
    return value.map((item) => normalizeValue(item, context));
  }
  if (value !== null && typeof value === "object") {
    const record = value as Record<string, unknown>;
    const dropCommandFingerprint = isCommandFingerprintObject(record);
    const entries = Object.entries(record).filter(
      ([key]) => key !== DROPPED_STACK_KEY && !(dropCommandFingerprint && key === "hash"),
    );
    return Object.fromEntries(
      entries.map(([key, inner]) => {
        if (TRANSPORT_HEADER_KEYS.has(key)) return [key, "<headers>"];
        if (VOLATILE_ID_LIST_KEYS.has(key)) return [key, "<ids>"];
        if (ID_CONTAINER_KEYS.has(key)) return [key, normalizeIdContainer(inner)];
        if (VOLATILE_ID_KEYS.has(key)) return [key, "<id>"];
        if (ENVIRONMENT_COUNT_KEYS.has(key)) return [key, "<count>"];
        if (TIME_KEYS.test(key) && inner !== null && inner !== undefined) return [key, "<time>"];
        return [key, normalizeValue(inner, context)];
      }),
    );
  }
  return value;
}

/** 第一层投影：按 `toolCallId` 折叠事件，保持首次出现顺序。 */
export function projectToolCalls(
  events: readonly EvalEvent[],
  context: NormalizeContext,
): ToolCall[] {
  const streamedInputs = new Map<string, unknown>();
  for (const event of events) {
    if (event.type !== "model.streaming") continue;
    const payload = asRecord(event.payload);
    if (!payload || payload.kind !== "tool_call") continue;
    streamedInputs.set(String(payload.toolCallId ?? ""), payload.input ?? null);
  }

  const calls: ToolCall[] = [];
  const byId = new Map<string, ToolCall>();
  for (const event of events) {
    if (event.type !== "tool.updated") continue;
    const payload = asRecord(event.payload);
    if (!payload) continue;
    const kind = payload.kind;
    if (kind === "batch" || kind === "raw" || kind === "progress") continue;
    const toolCallId = String(payload.toolCallId ?? "");
    if (kind === "scheduled") {
      const call: ToolCall = {
        name: String(payload.toolName ?? "<unknown>"),
        input: normalizeValue(payload.input ?? streamedInputs.get(toolCallId) ?? null, context),
        ok: false,
        output: "",
      };
      byId.set(toolCallId, call);
      calls.push(call);
      continue;
    }
    if (kind === "started") continue;
    const existing = byId.get(toolCallId);
    if (!existing) continue;
    if (kind === "result") {
      const result = asRecord(payload.result) ?? {};
      existing.ok = result.success === true;
      existing.output = normalizeValue(String(result.content ?? ""), context) as string;
      continue;
    }
    if (kind === "error") {
      const error = asRecord(payload.error) ?? {};
      existing.ok = false;
      existing.output = normalizeValue(String(error.message ?? ""), context) as string;
    }
  }
  return calls;
}

/** 第三层投影：折叠无序组 + 逐事件规范化。 */
export function projectEventStream(
  events: readonly EvalEvent[],
  context: NormalizeContext,
): ProjectedEvent[] {
  const folded = foldUnorderedRuns(events, (payload) =>
    stableStringify(normalizeValue(payload ?? null, context)),
  );
  return folded.map((event) => ({
    type: event.type,
    payload: normalizeValue(event.payload ?? null, context),
  }));
}

/**
 * 连续同类型段判定为无序组：状态推送；或「整段都是完成类事件且涉及 ≥2 个不同工具调用」的
 * 并行批次完成段（谁先跑完是竞态）。
 */
function isUnorderedRun(run: readonly EvalEvent[]): boolean {
  const first = run[0];
  if (!first) return false;
  if (UNORDERED_EVENT_TYPES.has(first.type)) return true;
  if (first.type !== "tool.updated") return false;
  const toolCallIds = new Set<string>();
  for (const event of run) {
    const payload = asRecord(event.payload);
    if (!payload) return false;
    const kind = payload.kind;
    if (kind !== "result" && kind !== "error" && kind !== "batch") return false;
    if (typeof payload.toolCallId === "string") toolCallIds.add(payload.toolCallId);
    const list = payload.toolCallIds;
    if (Array.isArray(list)) {
      for (const id of list) {
        if (typeof id === "string") toolCallIds.add(id);
      }
    }
  }
  return toolCallIds.size >= 2;
}

function foldUnorderedRuns(
  events: readonly EvalEvent[],
  sortKey: (payload: unknown) => string,
): EvalEvent[] {
  const folded: EvalEvent[] = [];
  let index = 0;
  while (index < events.length) {
    const current = events[index];
    if (!current) break;
    let end = index;
    while (end < events.length && events[end]?.type === current.type) {
      end += 1;
    }
    const run = events.slice(index, end);
    if (isUnorderedRun(run)) {
      folded.push(...[...run].sort((left, right) => sortKey(left.payload).localeCompare(sortKey(right.payload))));
    } else {
      folded.push(...run);
    }
    index = end;
  }
  return folded;
}

function asRecord(value: unknown): Record<string, unknown> | undefined {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}
