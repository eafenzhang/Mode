// C1 Task 0：会话生命周期探针的共享投影/规范化（只读观察，不读旧实现）。
//
// 职责边界：本文件只做「把 oracle 帧与 db.sqlite 行投影成可复现的 fixture 形状」；
// 运行/驱动/回放在 `probe-session-lifecycle.mts`。拆文件是为了守住 oxlint 的 max-lines。
import { DatabaseSync } from "node:sqlite";
import { fileURLToPath } from "node:url";

export type JsonObject = Record<string, unknown>;
export type PathContext = { dataRoot: string; cwd: string; tmpDir: string };

export const FIXTURE_DIR = fileURLToPath(new URL("./fixtures/session-lifecycle/", import.meta.url));

const IGNORED_NOTIFICATIONS = new Set([
  "computer-use/operation-event",
  "v4/telemetry/event",
  "process/mcpTelemetry",
  "process/mcpResourceSamples",
  "process/toolExecResource",
  "process/resourceSample",
  "plugins/operationProgress",
]);
const ID_KEYS = new Set([
  "id",
  "eventId",
  "sessionId",
  "childSessionId",
  "turnId",
  "messageId",
  "messageID",
  "assistantMessageId",
  "parentMessageId",
  "parentID",
  "anchorId",
  "anchorMessageId",
  "summaryMessageId",
  "targetMessageId",
  "toolMessageId",
  "toolCallId",
  "callId",
  "callID",
  "requestId",
  "inputId",
  "queryId",
  "traceId",
  "spanId",
  "parentSpanId",
  "attemptId",
  "databaseId",
  "operationId",
  "logicalCallId",
  "resultPartId",
  "checkpointId",
  "boundaryId",
  "boundaryMessageId",
  "mcpId",
  "mcpInstanceId",
  "session_id",
  "message_id",
  "tool_call_id",
  "turn_id",
  "trace_id",
  "span_id",
  "logical_request_id",
  "assistant_message_id",
  "parent_user_message_id",
  "user_message_id",
  "target_id",
  "checkpoint_id",
  "slug",
]);
const ID_LIST_KEYS = new Set([
  "orderedMessageIds",
  "toolCallIds",
  "committedToolCallIds",
  "pendingRequestIds",
  "requestIds",
]);
const ID_CONTAINER_KEYS = new Set(["parallelGroups", "executionOrder"]);
const TIME_KEYS = /^(timestamp|duration|date|elapsedMs|time|time_.*|.*At|.*Ms|.*_at|.*_ms)$/u;
const ENV_KEYS: Record<string, string> = {
  nodeVersion: "<node-version>",
  platform: "<platform>",
  osVersion: "<os-version>",
  shell: "<shell>",
  dialect: "<shell-dialect>",
};
const UUID_RE = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/giu;
const LOOPBACK_RE = /\b(?:https?|wss?):\/\/(?:127\.0\.0\.1|localhost|\[::1\]):\d+/gu;

function jsonEscaped(value: string): string {
  return JSON.stringify(value).slice(1, -1);
}

/** 机器相关值折叠：路径、id、时间、shell/env；结构（键、数组长度、顺序）不动。 */
export function normalizeString(value: string, context: PathContext): string {
  let text = value;
  for (const [raw, token] of [
    [context.dataRoot, "<data-root>"],
    [context.cwd, "<cwd>"],
    [context.tmpDir, "<tmp>"],
  ] as const) {
    text = text.replaceAll(raw, token);
    const escaped = jsonEscaped(raw);
    if (escaped !== raw) text = text.replaceAll(escaped, token);
  }
  return text
    .replace(/sess_[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/giu, "<session-id>")
    .replace(
      /msg_(?:part_)?[a-z0-9]+_[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}(?:_message)?/giu,
      "<message-id>",
    )
    .replace(
      /part_[a-z0-9]+_[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/giu,
      "<part-id>",
    )
    .replace(/turn_[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/giu, "<turn-id>")
    .replace(/toolu_mock_\d+_\d+/gu, "<tool-call-id>")
    .replace(/proj_[a-z0-9-]+/gu, "<project-id>")
    .replace(/\bmtime:\d+:size:\d+\b/gu, "<revision>")
    .replace(/[A-Za-z]:[\\/][^"]*?bash\.exe/giu, "<bash-path>")
    .replace(UUID_RE, "<id>")
    .replace(LOOPBACK_RE, "<loopback-url>");
}

function normalizeIdContainer(value: unknown): unknown {
  if (typeof value === "string") return "<id>";
  if (Array.isArray(value)) return value.map((item) => normalizeIdContainer(item));
  return value;
}

export type NormalizeDeepOptions = {
  /**
   * 这些键的数值原样保留（C3 的重试退避/目标计时需要区分 0 与正数；
   * C1/C2 不传时行为不变）。
   */
  preserveNumericKeys?: ReadonlySet<string>;
  /**
   * 这些键的原始值不规范化（C3 需要先保留目标/子会话 id，再映射成可比较的别名；
   * C1/C2 不传时行为不变）。
   */
  preserveValueKeys?: ReadonlySet<string>;
};

export function normalizeDeep(
  value: unknown,
  context: PathContext,
  options: NormalizeDeepOptions = {},
): unknown {
  if (typeof value === "string") return normalizeString(value, context);
  if (Array.isArray(value)) return value.map((item) => normalizeDeep(item, context, options));
  if (value === null || typeof value !== "object") return value;
  const entries = Object.entries(value as JsonObject).map(([key, inner]) => {
    if (options.preserveValueKeys?.has(key)) return [key, inner];
    if (options.preserveNumericKeys?.has(key) && typeof inner === "number") {
      return [key, inner];
    }
    if (ID_LIST_KEYS.has(key)) {
      return [key, Array.isArray(inner) ? inner.map(() => "<id>") : inner];
    }
    if (ID_CONTAINER_KEYS.has(key)) return [key, normalizeIdContainer(inner)];
    if (key === "tools" && inner !== null && typeof inner === "object" && !Array.isArray(inner)) {
      return [key, "<tool-flags>"];
    }
    if (key === "requestHeaders" || key === "responseHeaders") return [key, "<headers>"];
    if (key === "toolCount" || key === "chars" || key === "inputByteLength") {
      return [key, "<count>"];
    }
    if (key === "project_id") return [key, "<project-id>"];
    if (key === "revisionId") return [key, "<revision>"];
    if (key === "time" && inner !== null && typeof inner === "object" && !Array.isArray(inner)) {
      return [
        key,
        Object.fromEntries(Object.keys(inner as JsonObject).map((sub) => [sub, "<time>"])),
      ];
    }
    if (ENV_KEYS[key] !== undefined) return [key, ENV_KEYS[key]];
    if (ID_KEYS.has(key)) return [key, "<id>"];
    if (TIME_KEYS.test(key) && inner !== null && inner !== undefined) return [key, "<time>"];
    return [key, normalizeDeep(inner, context, options)];
  });
  return Object.fromEntries(entries);
}

function normalizeBashShellEntry(row: JsonObject): JsonObject {
  if (row.type !== "runtime/bash_shell_selection") return row;
  const data = (row.data ?? {}) as JsonObject;
  return {
    ...row,
    data: {
      ...data,
      dialect: "<shell-dialect>",
      display: { name: "<shell-name>" },
      label: "<shell-label>",
      path: "<bash-path>",
    },
  };
}

const JSON_COLUMNS: Record<string, string[]> = {
  session: ["permission"],
  message: ["data"],
  part: ["data"],
  session_input: ["payload"],
  session_entry: ["data"],
};

function normalizeDbRow(table: string, row: JsonObject, context: PathContext): JsonObject {
  const parsed: JsonObject = { ...row };
  for (const column of JSON_COLUMNS[table] ?? []) {
    if (typeof parsed[column] === "string") {
      parsed[column] = JSON.parse(parsed[column] as string);
    }
  }
  const normalized = normalizeDeep(parsed, context) as JsonObject;
  return table === "session_entry" ? normalizeBashShellEntry(normalized) : normalized;
}

export function readDb(path: string, context: PathContext): JsonObject {
  const db = new DatabaseSync(path, { readOnly: true });
  try {
    const tables: JsonObject = {};
    for (const table of ["session", "message", "part", "session_input", "session_entry"]) {
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
        rows: rows.map((row) => normalizeDbRow(table, row, context)),
      };
    }
    return { tables };
  } finally {
    db.close();
  }
}

export type Capture = {
  timeline: JsonObject[];
  events: JsonObject[];
  requests: JsonObject[];
  reverseRequests: JsonObject[];
  startupStorage: JsonObject[];
  ignoredNotifications: Record<string, number>;
  boundaries: Record<string, number>;
};

export function capture(): Capture {
  return {
    timeline: [],
    events: [],
    requests: [],
    reverseRequests: [],
    startupStorage: [],
    ignoredNotifications: {},
    boundaries: {},
  };
}

export function recordFrames(
  captureState: Capture,
  context: PathContext,
): { onWrite: (frame: JsonObject) => void; onFrame: (frame: JsonObject) => void } {
  const pending = new Map<number, { method: string; params: unknown }>();
  const normalize = (value: unknown): unknown => normalizeDeep(value, context);
  return {
    onWrite(frame: JsonObject) {
      if (typeof frame.method !== "string" || frame.id === undefined) return;
      const id = Number(frame.id);
      pending.set(id, { method: frame.method, params: frame.params });
      captureState.timeline.push({
        direction: "host->runtime",
        method: frame.method,
        params: normalize(frame.params),
      });
    },
    onFrame(frame: JsonObject) {
      const method = typeof frame.method === "string" ? frame.method : undefined;
      if (method && frame.id !== undefined) {
        const params = normalize(frame.params);
        captureState.reverseRequests.push({ method, params });
        captureState.timeline.push({ direction: "runtime->host", method, params });
        return;
      }
      if (method === "session/event") {
        const params = frame.params as JsonObject;
        captureState.events.push(normalize(params) as JsonObject);
        captureState.timeline.push({
          direction: "runtime->host",
          method,
          eventType: params.type,
          seq: params.seq,
        });
        return;
      }
      if (method === "startup/storageState") {
        const bootstrap = normalize(frame.params) as JsonObject;
        captureState.startupStorage.push(bootstrap);
        captureState.timeline.push({
          direction: "runtime->host",
          method,
          phase: bootstrap.phase,
          sequence: bootstrap.sequence,
        });
        return;
      }
      if (method === "state.updated") {
        const params = normalize(frame.params) as JsonObject;
        captureState.timeline.push({
          direction: "runtime->host",
          method,
          reason: params.reason,
          patch: params.patch,
        });
        return;
      }
      if (method) {
        if (IGNORED_NOTIFICATIONS.has(method)) {
          captureState.ignoredNotifications[method] =
            (captureState.ignoredNotifications[method] ?? 0) + 1;
          return;
        }
        captureState.timeline.push({
          direction: "runtime->host",
          method,
          params: normalize(frame.params),
        });
        return;
      }
      if (frame.id === undefined) return;
      const request = pending.get(Number(frame.id));
      if (request === undefined) return;
      pending.delete(Number(frame.id));
      captureState.requests.push({
        method: request.method,
        params: normalize(request.params),
        result: frame.error !== undefined ? { error: frame.error } : normalize(frame.result),
      });
    },
  };
}
