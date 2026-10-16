// M2 Task 0：对拍用的观测投影与规范化（spec §2 的「投影」、§3 的「合法差异」白名单）。
//
// 本模块是白名单规则的**唯一实现**（spec §3 末段）：比较器不重复一套规则，
// Rust 侧用例也按同一份规则把结果投影成同一形状后做精确相等断言。
import { createHash } from "node:crypto";
import { readdir, readFile, stat } from "node:fs/promises";
import { join, relative, sep } from "node:path";

import type { OracleEvent } from "./driver.mjs";

/** 三层比较里的第一层：一次工具调用的可比较形状。 */
export type ToolCall = {
  name: string;
  input: unknown;
  ok: boolean;
  output: string;
};

/** 第二层：`相对路径（posix 分隔） → 内容 sha256`。只比内容，不比 mtime / 权限。 */
export type FileSnapshot = Record<string, string>;

/** 第三层：规范化后的事件。 */
export type NormalizedEvent = { type: string; payload: unknown };

export type NormalizeContext = {
  dataRoot: string;
  cwd: string;
  /** 系统临时目录；隔离根与工作目录都在它下面，替换顺序必须从长到短。 */
  tmpDir: string;
};

/**
 * 状态推送类型：并发发出的相对顺序不是行为（spec §3「无序组」）。
 * 并行批次的完成事件也属于无序组，但判定更窄——见 `isUnorderedRun`。
 */
export const UNORDERED_EVENT_TYPES: readonly string[] = ["session.updated"];

/**
 * 非会话事件帧（CLI 的通知面，观测于 2026-10-16）：夹具只录会话事件，
 * 启动帧/遥测帧只用于诊断。这里是**排除**表而不是包含表——新出现的未列举帧会直接
 * 出现在夹具里供评审，而不是被静默丢掉（静默丢会让「协议新增了帧」这件事没人发现）。
 */
export const NON_SESSION_FRAME_TYPES: readonly string[] = [
  "startup/storageState",
  "state.updated",
  "v4/telemetry/event",
  "v4/conversation/frame",
  "process/mcpTelemetry",
  "process/mcpResourceSamples",
  "process/toolExecResource",
  "process/resourceSample",
  "plugins/operationProgress",
  "computer-use/operation-event",
  "interaction/providerRuntimeHeadersCancelled",
];

/** 从「所有帧」里挑出会话事件帧（`session/event` 通知映射而来的那些）。 */
export function pickSessionEvents(frames: readonly OracleEvent[]): OracleEvent[] {
  return frames.filter((frame) => !NON_SESSION_FRAME_TYPES.includes(frame.type));
}

/** 每次运行必然不同的 id 键（显式列举，避免把 providerId/modelId 这类事实键一起抹掉）。 */
const VOLATILE_ID_KEYS = new Set([
  "id",
  "eventId",
  "sessionId",
  "childSessionId",
  "turnId",
  "messageId",
  // 同一个事实在协议里有两个拼法（`messageID` 是历史遗留），两个都要列。
  "messageID",
  "assistantMessageId",
  "parentMessageId",
  // `anchorId` 形如 `msg_<随机>_<uuid>:<toolCallId>:tool-result`：
  // 值里的 uuid 会被替换，但随机的 `msg_…_` 前缀只能靠键名折叠（见 fixtures.test.mts 的守卫）。
  "anchorId",
  "anchorMessageId",
  "summaryMessageId",
  // Task 3 追加：`checkpoint.created` 的回指字段。值与 `messageId` 同类
  // （`msg_<base36 时间戳>_<uuid>`），前缀不是 uuid 形状、正文里的 uuid 折叠规不吃它，
  // 所以只能靠键名折叠——与上面 `anchorId` 同一条道理。
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

const VOLATILE_ID_LIST_KEYS = new Set([
  "toolCallIds",
  "pendingRequestIds",
  "requestIds",
  "committedToolCallIds",
]);

/**
 * id 容器键：容器的**结构**是行为（并行分组数、每组条数、执行顺序的条数），
 * 元素身份不是。只把最内层的字符串元素换成 `<id>`。
 *
 * 为什么不用「值里见到 `call_xxx` 就换掉」这种按形状匹配的规则：它会误伤工具输出正文
 * （`Read`/`Grep` 的结果里出现 `part_number_2`、`call_site_info` 这类普通文本是必然的），
 * 而正文差异恰恰是 M2 只读工具要对拍的东西（spec §3 的 id 规则只认 uuid 与 id 容器）。
 */
const ID_CONTAINER_KEYS = new Set(["parallelGroups", "executionOrder"]);

/** 时间键：值一律换成 `<time>`（`date` 是 HTTP 响应头里的当前时刻）。 */
const TIME_KEYS = /^(timestamp|duration|date|elapsedMs|.*At|.*Ms)$/;

/** 随机器变化的计数（装了哪些 MCP/插件/技能不是被测量行为，spec §3 末行）。 */
const ENVIRONMENT_COUNT_KEYS = new Set(["toolCount", "chars"]);

/**
 * 传输层请求/响应头：版本号、平台、时区、当前时刻都在里面，是环境事实而不是运行时行为。
 * 头**数量**（`requestHeaderCount` / `responseHeaderCount`）保留，头的值整体折叠。
 */
const TRANSPORT_HEADER_KEYS = new Set(["requestHeaders", "responseHeaders"]);

/** 全局标志：一段文本里可能有多个 id（`…:call_x:call_y`），只换第一个会漏。 */
const UUID_PATTERN = /[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}/g;
const LOOPBACK_URL_PATTERN = /\b(?:https?|wss?):\/\/(?:127\.0\.0\.1|localhost|\[::1\]):\d+/g;

/** 稳定序列化：键递归排序，用于比较与排序键。 */
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

function normalizeString(value: string, context: NormalizeContext): string {
  let result = value;
  // 路径：从长到短替换，短前缀不会把长路径切坏。
  // 两种形态都要换：事件里的**字段值**是单反斜杠，而 `model.streaming` 的 `delta` 是
  // 工具入参的 JSON **文本**，里面的路径是双反斜杠（JSON 转义）。只换一种就会漏掉另一种。
  for (const [raw, token] of pathTokens(context)) {
    result = result.replaceAll(raw, token);
    const escaped = jsonEscaped(raw);
    if (escaped !== raw) {
      result = result.replaceAll(escaped, token);
    }
  }
  result = result.replace(LOOPBACK_URL_PATTERN, "<loopback-url>");
  // 平铺在正文里的 uuid（`sess_<uuid>`、`msg_xxx_<uuid>:…`）折叠成标记。
  // **只认 uuid 形状**，不做「见到 `xxx_yyy` 就当 id」的按形状匹配：那会把工具输出正文
  // （`part_number_2`、`call_site_info`、`msg_hello.txt`）一起吃掉，
  // 而正文差异正是 M2 只读工具要对拍的东西。
  result = result.replace(UUID_PATTERN, "<id>");
  return result;
}

/** 长路径在前：短前缀不会把长路径切坏。 */
function pathTokens(context: NormalizeContext): [string, string][] {
  return [
    [context.dataRoot, "<data-root>"],
    [context.cwd, "<cwd>"],
    [context.tmpDir, "<tmp>"],
  ];
}

/** JSON 文本里的转义形态（Windows 路径的双反斜杠）。 */
function jsonEscaped(value: string): string {
  return JSON.stringify(value).slice(1, -1);
}

/** id 容器内部：字符串元素就是 id，整体换成 `<id>`，结构（分组/顺序的形状）保留。 */
function normalizeIdContainer(value: unknown): unknown {
  if (typeof value === "string") return "<id>";
  if (Array.isArray(value)) return value.map((item) => normalizeIdContainer(item));
  return value;
}

/** 按 spec §3 的白名单规范化一个 payload。 */
export function normalizeValue(value: unknown, context: NormalizeContext): unknown {
  if (typeof value === "string") return normalizeString(value, context);
  if (Array.isArray(value)) {
    // 注意：**不排序**。数组顺序本身常常是行为（调度顺序、执行顺序），
    // 顺序差异只能由 §3 的「无序组」显式豁免，不能在这里一刀切掉。
    return value.map((item) => normalizeValue(item, context));
  }
  if (value !== null && typeof value === "object") {
    const entries = Object.entries(value as Record<string, unknown>);
    return Object.fromEntries(
      entries.map(([key, inner]) => {
        if (TRANSPORT_HEADER_KEYS.has(key)) return [key, "<headers>"];
        if (VOLATILE_ID_LIST_KEYS.has(key)) return [key, "<ids>"];
        if (ID_CONTAINER_KEYS.has(key)) return [key, normalizeIdContainer(inner)];
        if (VOLATILE_ID_KEYS.has(key)) return [key, "<id>"];
        if (ENVIRONMENT_COUNT_KEYS.has(key)) return [key, "<count>"];
        // 时间键不限数值：`timestamp` / `committedAt` 是 ISO 字符串，`durationMs` 才是数值。
        if (TIME_KEYS.test(key) && inner !== null && inner !== undefined) return [key, "<time>"];
        return [key, normalizeValue(inner, context)];
      }),
    );
  }
  return value;
}

/**
 * 第一层投影：按 `toolCallId` 折叠事件，保持首次出现顺序。
 *
 * 实测事实（2026-10-16）：`tool.updated` 的 `scheduled` 事件**故意不带 input**
 * （`inputOmitted:true` + `inputRef:"model_stream"`），调用入参只出现在
 * `model.streaming` 的 `kind:"tool_call"` 事件里。入参因此必须从那里取，
 * 否则第一层会拿 `null` 跟 `null` 比，工具入参整整一层失去意义。
 */
export function projectToolCalls(
  events: readonly OracleEvent[],
  context: NormalizeContext,
): ToolCall[] {
  const streamedInputs = new Map<string, unknown>();
  for (const event of events) {
    if (event.type !== "model.streaming") continue;
    const payload = (event.payload ?? {}) as Record<string, unknown>;
    if (payload.kind !== "tool_call") continue;
    streamedInputs.set(String(payload.toolCallId ?? ""), payload.input ?? null);
  }

  const calls: ToolCall[] = [];
  const byId = new Map<string, ToolCall>();
  for (const event of events) {
    if (event.type !== "tool.updated") continue;
    const payload = (event.payload ?? {}) as Record<string, unknown>;
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
      const result = (payload.result ?? {}) as Record<string, unknown>;
      existing.ok = result.success === true;
      existing.output = normalizeString(String(result.content ?? ""), context);
      continue;
    }
    if (kind === "error") {
      const error = (payload.error ?? {}) as Record<string, unknown>;
      existing.ok = false;
      existing.output = normalizeString(String(error.message ?? ""), context);
    }
  }
  return calls;
}

/** 第三层投影：折叠无序组 + 逐事件规范化。 */
export function projectEventStream(
  events: readonly OracleEvent[],
  context: NormalizeContext,
): NormalizedEvent[] {
  // 先折叠（折叠要读**原始** payload 才能分辨「哪几个事件属于同一次工具调用」），
  // 再规范化。排序键用规范化后的值，否则键里会带上随机 id，两次运行的排序结果可能不同。
  const folded = foldUnorderedRuns(events, (payload) =>
    stableStringify(normalizeValue(payload ?? null, context)),
  );
  return folded.map((event) => ({
    type: event.type,
    payload: normalizeValue(event.payload ?? null, context),
  }));
}

/**
 * 把一个**连续同类型**段判定为无序组（spec §3「无序组」）：
 *
 * - 状态推送（`UNORDERED_EVENT_TYPES`）：并发发出的相对顺序不是行为；
 * - 并行批次的完成事件：同一个 assistant 消息里的多个工具调用会被并行执行，
 *   谁先跑完是竞态。只在「整段都是完成类事件（result/error/batch）**且**涉及 ≥2 个不同
 *   工具调用」时才成立——单次调用的 `result`/`batch` 先后、`scheduled`/`started` 的调度顺序
 *   都不折叠（那是行为）。
 */
function isUnorderedRun(run: readonly OracleEvent[]): boolean {
  const first = run[0];
  if (!first) return false;
  if (UNORDERED_EVENT_TYPES.includes(first.type)) return true;
  if (first.type !== "tool.updated") return false;
  const toolCallIds = new Set<string>();
  for (const event of run) {
    const payload = (event.payload ?? {}) as Record<string, unknown>;
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

/** 连续同类型的无序段折叠成按稳定串排序的集合。 */
function foldUnorderedRuns(
  events: readonly OracleEvent[],
  sortKey: (payload: unknown) => string,
): OracleEvent[] {
  const folded: OracleEvent[] = [];
  let index = 0;
  while (index < events.length) {
    const current = events[index] as OracleEvent;
    let end = index;
    while (end < events.length && events[end]?.type === current.type) {
      end += 1;
    }
    const run = events.slice(index, end);
    if (isUnorderedRun(run)) {
      folded.push(
        ...[...run].sort((left, right) => {
          const a = sortKey(left.payload);
          const b = sortKey(right.payload);
          return a < b ? -1 : a > b ? 1 : 0;
        }),
      );
    } else {
      folded.push(...run);
    }
    index = end;
  }
  return folded;
}

const SNAPSHOT_SKIP_DIRS = new Set([".git", "node_modules", ".mode"]);

/** 第二层投影：目录内容快照。 */
export async function takeFileSnapshot(
  directory: string,
  context: NormalizeContext,
): Promise<FileSnapshot> {
  const snapshot: FileSnapshot = {};
  const walk = async (current: string): Promise<void> => {
    const entries = await readdir(current, { withFileTypes: true });
    for (const entry of entries) {
      const absolute = join(current, entry.name);
      if (entry.isDirectory()) {
        if (SNAPSHOT_SKIP_DIRS.has(entry.name)) continue;
        if (absolute.startsWith(context.dataRoot)) continue;
        await walk(absolute);
        continue;
      }
      if (!entry.isFile()) continue;
      const info = await stat(absolute);
      if (!info.isFile()) continue;
      const content = await readFile(absolute);
      const key = relative(directory, absolute).split(sep).join("/");
      snapshot[key] = createHash("sha256").update(content).digest("hex");
    }
  };
  await walk(directory);
  return Object.fromEntries(
    Object.entries(snapshot).sort(([left], [right]) => (left < right ? -1 : 1)),
  );
}
