// C2 Task 8：compact 回放的共享比较面（`replay-c2-compact.mts` 的投影半边；
// 拆文件守住 oxlint 的 `max-lines`，不是第二套纪律——所有规则登记都在
// `replay-c2.mts` 文件头与 `tauri-agent-runtime.md` §C2.8）。
import { normalizeDeep, type JsonObject } from "./session-lifecycle-fixtures.mjs";
import {
  diffDbRows,
  diffDeep,
  foldRegisteredMessageIds,
  type C2Diff,
} from "./replay-c2.mjs";

export type DbSnapshot = Record<string, JsonObject & { rowCount?: number; rows?: JsonObject[] }>;

export function rowsOf(db: DbSnapshot, table: string): JsonObject[] {
  return (db[table]?.rows as JsonObject[]) ?? [];
}

/** 快照 session 的登记折叠：title/titleSource 是 C1 title 缺省（登记缺省事件类型）的
 * 快照/DB 物化，两侧同删。 */
export function foldSnapshotSession(session: JsonObject | null): JsonObject | null {
  if (session === null || typeof session !== "object") return session;
  const folded = { ...session };
  delete folded.title;
  delete folded.titleSource;
  return folded;
}

/** 快照 runtime 的登记折叠（§4.5 差异表 #1/#2/#3/#4）：eventSeq/stateRevision 绝对值
 * 是运行时起点事实；deliveryKind 不产出；contextUsage.breakdown 不产出。 */
export function foldSnapshotRuntime(runtime: JsonObject | null): JsonObject | null {
  if (runtime === null || typeof runtime !== "object") return runtime;
  const folded: JsonObject = { ...runtime };
  delete folded.eventSeq;
  delete folded.stateRevision;
  delete folded.deliveryKind;
  const usage = folded.contextUsage;
  if (usage !== null && typeof usage === "object") {
    const usageFolded: JsonObject = { ...(usage as JsonObject) };
    delete usageFolded.breakdown;
    folded.contextUsage = usageFolded;
  }
  return folded;
}

/** session 行的登记折叠：title/title_source/time_title_updated（title 缺省的物化）。 */
export function foldSessionRow(row: JsonObject | null): JsonObject | null {
  if (row === null || typeof row !== "object") return row;
  const folded = { ...row };
  delete folded.title;
  delete folded.title_source;
  delete folded.time_title_updated;
  return folded;
}

/** 增量行的位置字段折叠（行号绝对值随会话 timeline 行存在与否漂移，登记折叠）。 */
export function foldRowPosition(row: JsonObject): JsonObject {
  const folded = { ...row };
  if ("sequence" in folded) folded.sequence = "<row-position>";
  if ("messageSequence" in folded) folded.messageSequence = "<row-position>";
  return folded;
}

/** 受理应答的可比子集（§1.1/§1.9；快照面按 §4.5 差异折叠）。 */
export function acceptedResponseDiffs(
  result: JsonObject,
  expectCompactResponse: JsonObject,
  path: string,
  normalizeActual: (value: unknown) => unknown,
): C2Diff[] {
  const diffs: C2Diff[] = [];
  // 实际侧先过共享折叠（normalizeDeep；夹具侧是录制时的同一函数输出，不再二次投）。
  // 生成的 id 形状键（currentTurnId 等，契约 §1.9）两侧同折。
  const normalized = foldRegisteredMessageIds(normalizeActual(result)) as JsonObject;
  const compact = (normalized.compact ?? null) as JsonObject | null;
  const expectCompact = (expectCompactResponse.compact ?? null) as JsonObject | null;
  if (result.response !== "") {
    diffs.push({ layer: "response", path: `${path}.response`, message: "response 恒空串", actual: result.response, expected: "" });
  }
  const actualKeys = Object.keys((result.compact ?? {}) as JsonObject).sort();
  const expectedKeys = (expectCompactResponse.compactKeys as string[] | undefined) ?? undefined;
  if (expectedKeys !== undefined) {
    diffs.push(...diffDeep("response", `${path}.compactKeys`, actualKeys, [...expectedKeys].sort()));
  }
  diffs.push(...diffDeep("response", `${path}.compact`, compact, expectCompact));
  const snapshot = (normalized.snapshot ?? null) as JsonObject | null;
  const expectSnapshot = (expectCompactResponse.snapshot ?? null) as JsonObject | null;
  if (expectSnapshot === null) {
    // 失败夹具的录制面只到 `snapshotRuntime`（probe-c2-compact-failure 的投影）：
    // 比折叠后的 runtime 子面，其余快照键不比（登记）。
    const expectRuntime = (expectCompactResponse.snapshotRuntime ?? null) as JsonObject | null;
    diffs.push(...diffDeep("response", `${path}.snapshot.runtime`, foldSnapshotRuntime((snapshot?.runtime ?? null) as JsonObject | null), foldSnapshotRuntime(expectRuntime)));
    return diffs;
  }
  if (snapshot === null) {
    diffs.push({ layer: "response", path: `${path}.snapshot`, message: "snapshot 缺失", actual: "absent", expected: "present" });
    return diffs;
  }
  diffs.push(
    ...diffDeep("response", `${path}.snapshot.keys`, Object.keys((result.snapshot ?? {}) as JsonObject).sort(), (expectSnapshot.keys as string[]) ?? []),
  );
  diffs.push(...diffDeep("response", `${path}.snapshot.session`, foldSnapshotSession((snapshot.session ?? null) as JsonObject | null), foldSnapshotSession((expectSnapshot.session ?? null) as JsonObject | null)));
  diffs.push(...diffDeep("response", `${path}.snapshot.projection`, snapshot.projection ?? null, expectSnapshot.projection ?? null));
  diffs.push(...diffDeep("response", `${path}.snapshot.runtime`, foldSnapshotRuntime((snapshot.runtime ?? null) as JsonObject | null), foldSnapshotRuntime((expectSnapshot.runtime ?? null) as JsonObject | null)));
  return diffs;
}

/**
 * compact 增量行比较（§1.3 的观察主体）：
 * - message：按各侧 compact 前的行数切片（夹具侧 = `readBefore.messageCount`）；
 * - part：挂在增量 message 上的行（按 `messageSequence` 落在增量消息集合内）；
 * - turn_usage / model_usage：最后一行（compact 行；增量写法逐行追加）；
 * - tool_usage / session_input：整表（观察值恒空/不变）。
 *
 * 行的绝对 `sequence` / `messageSequence` 位置登记折叠：oracle 的会话起点
 * timeline 行（C1 时代未对齐面）让两侧的行号整体差 1——增量行按数组顺序对齐
 * 比较，行内序（part.sequence 0/1）仍然逐字比。
 */
export function diffCompactAddedRows(
  before: { messageCount: number; turnUsageCount: number; modelUsageCount: number },
  actualAfter: DbSnapshot,
  expectedAfter: DbSnapshot,
  expectedBeforeMessageCount: number,
): C2Diff[] {
  const diffs: C2Diff[] = [];
  const actualAdded = rowsOf(actualAfter, "message").slice(before.messageCount);
  const expectedAdded = rowsOf(expectedAfter, "message").slice(expectedBeforeMessageCount);
  diffs.push(...diffDbRows("dbAfterCompact.message.added", actualAdded.map(foldRowPosition), expectedAdded.map(foldRowPosition)));
  const addedSequences = new Set(actualAdded.map((row) => String(row.sequence)));
  const expectedAddedSequences = new Set(expectedAdded.map((row) => String(row.sequence)));
  const actualParts = rowsOf(actualAfter, "part")
    .filter((row) => addedSequences.has(String(row.messageSequence)))
    .map(foldRowPosition);
  const expectedParts = rowsOf(expectedAfter, "part")
    .filter((row) => expectedAddedSequences.has(String(row.messageSequence)))
    .map(foldRowPosition);
  diffs.push(...diffDbRows("dbAfterCompact.part.added", actualParts, expectedParts));
  diffs.push(...diffDbRows("dbAfterCompact.turnUsage.last", rowsOf(actualAfter, "turn_usage").at(-1) ?? null, rowsOf(expectedAfter, "turn_usage").at(-1) ?? null));
  diffs.push(...diffDbRows("dbAfterCompact.modelUsage.last", rowsOf(actualAfter, "model_usage").at(-1) ?? null, rowsOf(expectedAfter, "model_usage").at(-1) ?? null));
  diffs.push(...diffDbRows("dbAfterCompact.toolUsage", actualAfter.tool_usage ?? null, expectedAfter.tool_usage ?? null));
  diffs.push(...diffDbRows("dbAfterCompact.sessionInput", actualAfter.session_input ?? null, expectedAfter.session_input ?? null));
  return diffs;
}

/** compact 相关的行计数（C2 拥有的计数面；prompt 行形状见 §C2.8 DB 层未对齐面）。 */
export function summaryCounts(db: DbSnapshot): JsonObject {
  const messages = rowsOf(db, "message");
  const parts = rowsOf(db, "part");
  return {
    messages: db.message?.rowCount ?? null,
    compactSummaryMessages: messages.filter((row) => {
      const data = row.data as JsonObject;
      return ((data.semantics as JsonObject | undefined)?.kind as string) === "compact_summary";
    }).length,
    compactionParts: parts.filter((row) => ((row.data as JsonObject)?.type as string) === "compaction").length,
    failedTimelineMessages: messages.filter((row) => ((row.data as JsonObject)?.finish as string) === "failed").length,
    skippedTimelineMessages: messages.filter((row) => ((row.data as JsonObject)?.finish as string) === "skipped").length,
  };
}

/** 表形状比较：列定义（pragma）逐字段。行数/行内容不比（C1 时代未对齐面）。 */
export function diffTableShapes(path: string, actual: DbSnapshot, expected: DbSnapshot): C2Diff[] {
  const diffs: C2Diff[] = [];
  for (const table of Object.keys(expected)) {
    const expectedTable = expected[table];
    const actualTable = actual[table];
    if (actualTable === undefined) {
      diffs.push({ layer: "db", path: `${path}.${table}`, message: "实际缺少表", actual: undefined, expected: Object.keys(expectedTable ?? {}) });
      continue;
    }
    diffs.push(...diffDeep("db", `${path}.${table}.columns`, actualTable.columns ?? null, expectedTable.columns ?? null));
  }
  return diffs;
}

/** 从驱动层的错误文案里解析 JSON-RPC 错误帧（`agent-server-session` 的 reject 形状）。 */
export function parseRpcError(error: unknown): JsonObject | null {
  const message = error instanceof Error ? error.message : String(error);
  const marker = "失败：";
  const index = message.indexOf(marker);
  if (index < 0) return null;
  try {
    return JSON.parse(message.slice(index + marker.length)) as JsonObject;
  } catch {
    return null;
  }
}

// normalizeDeep/foldRegisteredMessageIds 的引用面收敛（比较入口统一走本文件）。
export { normalizeDeep };
