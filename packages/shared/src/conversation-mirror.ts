import { z } from "zod";

import type { ModeSessionImportHistory } from "./mode-protocol/index.js";
import type { ModeMessageWithParts } from "./mode-protocol-legacy-types.js";

/**
 * 远程工作区对话记录镜像（docs/specs/remote-workspace-conversation-mirror.md）。
 *
 * 镜像的产物是**真会话**：把对端机器在同一工作区路径下、本机本地 scope 产生的会话，
 * 经 `session/create` + `importedHistory`（`source: "remoteMirror"`）导入到目标 scope，
 * 因此它可续聊、可置顶 / 归档 / 删除、进任务搜索——与本地对话同质。
 *
 * 本模块只放投影层的纯函数：会话消息提取、来源筛选、有界截断、确定性目标 id 与导入 payload 构造。
 * 读写与编排由 Renderer 层通过 workspace 服务完成。
 */

export const CONVERSATION_MIRROR_IMPORT_SOURCE = "remoteMirror" as const;

/** 镜像会话 id 前缀：读取对端本地 scope 时按它跳过副本，避免回声与第三方扩散。 */
export const CONVERSATION_MIRROR_SESSION_ID_PREFIX = "mirror-";

/**
 * 快照载体（服务侧）：每台机器把自己在某个工作区路径下的本机会话发布成一份
 * 「origin = 自己」的快照，存在自己的数据目录里；对端连接时既发布自己的、也读取对端的。
 * 读取方按 originDeviceId 覆盖式幂等去重，并剔除自己的快照（防回声）。
 *
 * 与上面的导入投影（`buildConversationMirrorRecord` 等）是同一份数据的两面：
 * 快照是**传输与存储**形态，导入投影是**落进目标 scope 的会话**形态。
 */
export const CONVERSATION_MIRROR_VERSION = 1;

/** 数据根下 `<appConfigDir>/conversation-mirror/<workspacePathHash>/<originDeviceId>.json`。 */
export const CONVERSATION_MIRROR_DIR_NAME = "conversation-mirror";

export const CONVERSATION_MIRROR_LIMITS = {
  /**
   * 单轮处理的来源会话条数上限（按 updatedAt 降序取最新的前 N 条）。
   *
   * 实际窗口由调用方传入（侧栏分页窗口，见 `selectMirrorSourceTasks` 的 maxRecords 参数）：
   * 默认只同步最近 5 条，用户「展开更多」把窗口 +5 后，调用方按新窗口重新跑一轮。
   * 这里是硬上限，同时也是任务列表查询的读取上限基准。
   */
  maxRecords: 20,
  /** 单条会话最多导入的消息条数（保留尾部）。 */
  maxMessagesPerSession: 200,
  /** 单条消息最大字符数。 */
  maxTextCharsPerMessage: 4_000,
  /** 单条记录最大字节数：为远端 RPC 物理帧（1 MiB）留余量。 */
  maxRecordBytes: 256 * 1024,
  /** 单份快照最大字节数（与远端 RPC 分帧量级一致）。 */
  maxSnapshotBytes: 512 * 1024,
} as const;

export interface ConversationMirrorLimits {
  maxRecords: number;
  maxMessagesPerSession: number;
  maxTextCharsPerMessage: number;
  maxRecordBytes: number;
  maxSnapshotBytes: number;
}

const TRUNCATION_SUFFIX = "…";
const textEncoder = new TextEncoder();

const nonEmptyString = z.string().trim().min(1);
const timestampMs = z.number().int().nonnegative();

export const conversationMirrorMessageSchema = z
  .object({
    role: z.enum(["user", "assistant"]),
    text: z.string(),
    timestamp: timestampMs.optional(),
  })
  .strict();
export type ConversationMirrorMessage = z.infer<typeof conversationMirrorMessageSchema>;

export const conversationMirrorRecordSchema = z
  .object({
    sessionId: nonEmptyString,
    title: z.string(),
    createdAt: timestampMs,
    updatedAt: timestampMs,
    /** 来源会话真实消息条数；`messages` 可能因上限被截断。 */
    messageCount: z.number().int().nonnegative(),
    /** 内容是否被截断（消息条数 / 文本长度 / 字节预算）。 */
    truncated: z.boolean(),
    messages: z.array(conversationMirrorMessageSchema),
  })
  .strict();
export type ConversationMirrorRecord = z.infer<typeof conversationMirrorRecordSchema>;

export const conversationMirrorSnapshotSchema = z
  .object({
    version: z.literal(CONVERSATION_MIRROR_VERSION),
    originDeviceId: nonEmptyString,
    originLabel: nonEmptyString,
    generatedAt: timestampMs,
    /** 同步域：发布方当前工作区的路径。读写两侧按该路径的 hash 定位目录。 */
    workspacePath: nonEmptyString,
    records: z.array(conversationMirrorRecordSchema),
  })
  .strict();
export type ConversationMirrorSnapshot = z.infer<typeof conversationMirrorSnapshotSchema>;

/** 设备身份 + 展示名。读取方用它判断「这条快照是不是我自己的」。 */
export const conversationMirrorIdentitySchema = z
  .object({
    deviceId: nonEmptyString,
    label: nonEmptyString,
  })
  .strict();
export type ConversationMirrorIdentity = z.infer<typeof conversationMirrorIdentitySchema>;

export const conversationMirrorWorkspaceRefSchema = z
  .object({
    /** 同步域路径；本机与对端都用它定位镜像目录。 */
    workspacePath: nonEmptyString,
  })
  .strict();
export type ConversationMirrorWorkspaceRef = z.infer<typeof conversationMirrorWorkspaceRefSchema>;

export const conversationMirrorReadResultSchema = z
  .object({
    snapshots: z.array(conversationMirrorSnapshotSchema),
  })
  .strict();
export type ConversationMirrorReadResult = z.infer<typeof conversationMirrorReadResultSchema>;

export const conversationMirrorPublishResultSchema = z
  .object({
    /** 是否落盘（origin 与本机相同的快照会被拒绝，不构成错误）。 */
    stored: z.boolean(),
    reason: z.enum(["stored", "self-origin", "foreign-origin", "invalid-record"]).optional(),
    recordCount: z.number().int().nonnegative(),
  })
  .strict();
export type ConversationMirrorPublishResult = z.infer<typeof conversationMirrorPublishResultSchema>;

/**
 * 快照在接收侧的稳定标识：一个 origin 一份文件，重复发布覆盖。
 * 会话内容按 `sessionId` 在 record 层去重（同一 origin 的重复会话不叠加）。
 */
export function conversationMirrorSnapshotFileStem(originDeviceId: string): string {
  return originDeviceId.replaceAll(/[^A-Za-z0-9._-]/gu, "_");
}

/** 同一 origin 内按 sessionId 去重，保留 updatedAt 较新的一条。 */
export function dedupeConversationMirrorRecords(
  records: readonly ConversationMirrorRecord[],
): ConversationMirrorRecord[] {
  const bySession = new Map<string, ConversationMirrorRecord>();
  for (const record of records) {
    const existing = bySession.get(record.sessionId);
    if (!existing || record.updatedAt >= existing.updatedAt) {
      bySession.set(record.sessionId, record);
    }
  }
  return [...bySession.values()];
}

/** 快照的 UTF-8 字节长度（RPC 分帧按字节计，不能用字符串长度近似）。 */
export function conversationMirrorSnapshotByteSize(snapshot: ConversationMirrorSnapshot): number {
  return textEncoder.encode(JSON.stringify(snapshot)).length;
}

/**
 * 快照有界化：先按会话数/消息数/文本长度截断，再按字节预算从最旧会话开始丢弃。
 * 截断只影响镜像投影，不改变来源会话本身。
 */
export function boundConversationMirrorSnapshot(
  snapshot: ConversationMirrorSnapshot,
  limits: ConversationMirrorLimits = CONVERSATION_MIRROR_LIMITS,
): ConversationMirrorSnapshot {
  const sorted = dedupeConversationMirrorRecords(snapshot.records).sort(
    (left, right) => right.updatedAt - left.updatedAt,
  );
  let records = sorted
    .slice(0, limits.maxRecords)
    .map((record) => boundConversationMirrorRecord(record, limits));

  while (records.length > 1) {
    const candidate = { ...snapshot, records };
    if (conversationMirrorSnapshotByteSize(candidate) <= limits.maxSnapshotBytes) {
      break;
    }
    records = records.slice(0, -1);
  }

  return { ...snapshot, records };
}

/** 参与镜像的最小会话信息；`workspaceIdentity` 用于判定是不是本机本地 scope。 */
export interface ConversationMirrorSourceTask {
  taskId: string;
  title: string;
  createdAt: number;
  updatedAt: number;
  workspaceIdentity?: string;
}

/**
 * 镜像会话在目标 scope 里的确定性 id。只取决于来源会话 id，因此：
 * 重复同步命中同一个会话（upsert + 按 source 清理旧导入消息 → 幂等），
 * 重连换端口也不会产生重复；不同机器的会话 id 不会相同。
 *
 * 直接沿用来源 id 而不是做哈希：这是纯函数（Renderer 与 Host 都要用，不能依赖 node:crypto），
 * 也便于在日志/库里直接看出镜像来源会话。
 */
export function buildMirrorImportSessionId(originSessionId: string): string {
  return `${CONVERSATION_MIRROR_SESSION_ID_PREFIX}${originSessionId}`;
}

export function isMirrorSessionId(sessionId: string): boolean {
  return sessionId.startsWith(CONVERSATION_MIRROR_SESSION_ID_PREFIX);
}

/**
 * 只镜像本机本地 scope 产生的会话。
 *
 * 带 `workspaceIdentity` 的行是「以某个对端身份打开的工作区」的会话——它们的原始记录在对端机器上，
 * 再导入一次等于转发对端内容（不变量 2）。
 */
export function isMirrorEligibleTask(task: ConversationMirrorSourceTask): boolean {
  return !task.workspaceIdentity?.trim();
}

/** 按 updatedAt 取最新的前 N 条可镜像候选（记录本身还有消息数/字节上限兜底）。 */
export function selectMirrorSourceTasks(
  tasks: readonly ConversationMirrorSourceTask[],
  maxRecords: number = CONVERSATION_MIRROR_LIMITS.maxRecords,
): ConversationMirrorSourceTask[] {
  return tasks
    .filter((task) => isMirrorEligibleTask(task) && !isMirrorSessionId(task.taskId))
    .slice()
    .sort((left, right) => right.updatedAt - left.updatedAt)
    .slice(0, maxRecords);
}

/**
 * ②方向（本机记录 → 对端本地 scope）的来源筛选。
 *
 * 与①方向**必须不同**：这里要保留带 `workspaceIdentity` 的行。本机连上远端工作区后在其中发起的对话，
 * 落在「以对端身份打开的那个 scope」里（`{workspacePath, workspaceIdentity: 本机 attach 身份}`），
 * 只取不带 identity 的本机本地 scope 会让②方向在双机路径不同时恒为空——
 * 这就是「本机发起的远端对话，远端看不到」的成因。
 *
 * 镜像副本（`mirror-` 前缀）仍然跳过：那是①方向或第三方导入进来的内容，再转发一次就成环。
 */
export function selectOwnMirrorSourceTasks(
  tasks: readonly ConversationMirrorSourceTask[],
  maxRecords: number = CONVERSATION_MIRROR_LIMITS.maxRecords,
): ConversationMirrorSourceTask[] {
  return tasks
    .filter((task) => !isMirrorSessionId(task.taskId))
    .slice()
    .sort((left, right) => right.updatedAt - left.updatedAt)
    .slice(0, maxRecords);
}

/**
 * ②方向合并多份来源（本机本地 scope + 本机视图 scope）：按 taskId 去重并保留 updatedAt 较新的一条。
 * 两个 scope 分属不同机器，正常情况下不会有同 id；去重只为让重复读取不产生重复导入尝试。
 */
export function mergeMirrorSourceTasks(
  sources: ReadonlyArray<readonly ConversationMirrorSourceTask[]>,
): ConversationMirrorSourceTask[] {
  const byTaskId = new Map<string, ConversationMirrorSourceTask>();
  for (const tasks of sources) {
    for (const task of tasks) {
      const existing = byTaskId.get(task.taskId);
      if (!existing || task.updatedAt > existing.updatedAt) {
        byTaskId.set(task.taskId, task);
      }
    }
  }
  return [...byTaskId.values()];
}

/** 只取可读文本：工具输入输出、附件字节、隐藏的 synthetic 消息都不进镜像。 */
export function extractConversationMirrorMessages(
  messages: readonly ModeMessageWithParts[],
): ConversationMirrorMessage[] {
  const output: ConversationMirrorMessage[] = [];
  for (const message of messages) {
    const role = message.info.role;
    if (role !== "user" && role !== "assistant") {
      continue;
    }
    const semantics = message.info.semantics;
    if (semantics?.uiVisibility === "hidden" || semantics?.transcriptVisibility === "hidden") {
      continue;
    }
    const chunks: string[] = [];
    for (const part of message.parts) {
      if (part.type === "text") {
        chunks.push(part.text);
      }
    }
    const text = chunks.join("\n\n").trim();
    if (!text) {
      continue;
    }
    output.push({ role, text, timestamp: message.info.time?.created });
  }
  return output;
}

function boundMessageText(text: string, maxChars: number): string {
  return text.length <= maxChars ? text : `${text.slice(0, maxChars)}${TRUNCATION_SUFFIX}`;
}

/** UTF-8 字节长度（RPC 分帧按字节计，不能用字符串长度近似）。 */
export function conversationMirrorRecordByteSize(record: ConversationMirrorRecord): number {
  return textEncoder.encode(JSON.stringify(record)).length;
}

/**
 * 有界化：先按消息数与单条文本长度截断，再按字节预算从最旧消息开始丢弃。
 * 截断只影响镜像投影，不改变来源会话本身。
 */
export function boundConversationMirrorRecord(
  record: ConversationMirrorRecord,
  limits: ConversationMirrorLimits = CONVERSATION_MIRROR_LIMITS,
): ConversationMirrorRecord {
  const kept = record.messages.slice(-limits.maxMessagesPerSession);
  let messages = kept.map((message) => {
    const text = boundMessageText(message.text, limits.maxTextCharsPerMessage);
    return text === message.text ? message : { ...message, text };
  });
  let truncated =
    record.truncated ||
    kept.length < record.messages.length ||
    messages.some((message, index) => message.text !== kept[index]?.text);

  while (messages.length > 1) {
    const candidate = { ...record, messages };
    if (conversationMirrorRecordByteSize(candidate) <= limits.maxRecordBytes) {
      break;
    }
    messages = messages.slice(1);
    truncated = true;
  }

  return { ...record, messages, truncated };
}

export interface BuildConversationMirrorRecordParams {
  task: ConversationMirrorSourceTask;
  messages: readonly ModeMessageWithParts[];
  limits?: ConversationMirrorLimits;
}

/**
 * 由来源会话构建镜像记录。没有可读文本时返回 null：不导入空会话（否则目标机器会多出一条空对话）。
 */
export function buildConversationMirrorRecord(
  params: BuildConversationMirrorRecordParams,
): ConversationMirrorRecord | null {
  const extracted = extractConversationMirrorMessages(params.messages);
  if (extracted.length === 0) {
    return null;
  }
  return boundConversationMirrorRecord(
    {
      sessionId: params.task.taskId,
      title: params.task.title,
      createdAt: params.task.createdAt,
      updatedAt: params.task.updatedAt,
      messageCount: extracted.length,
      truncated: false,
      messages: extracted,
    },
    params.limits ?? CONVERSATION_MIRROR_LIMITS,
  );
}

/** 构造 `session/create` 的 importedHistory（`source: "remoteMirror"`）。 */
export function buildConversationMirrorImportedHistory(
  record: ConversationMirrorRecord,
): Extract<ModeSessionImportHistory, { source: typeof CONVERSATION_MIRROR_IMPORT_SOURCE }> {
  return {
    source: CONVERSATION_MIRROR_IMPORT_SOURCE,
    title: record.title.trim() || record.sessionId,
    createdAt: record.createdAt,
    updatedAt: record.updatedAt,
    originSessionId: record.sessionId,
    messages: record.messages.map((message) => ({
      role: message.role,
      content: message.text,
      ...(message.timestamp === undefined ? {} : { timestamp: message.timestamp }),
    })),
  };
}

/** 是否需要对这条来源记录执行导入：目标 scope 里已有同一镜像会话且不更旧时跳过。 */
export function shouldImportMirrorRecord(params: {
  record: ConversationMirrorRecord;
  targetUpdatedAt: number | null | undefined;
}): boolean {
  // 目标 task meta 的 updatedAt 由会话快照投影而来；用户续聊后它会前进，因此不会反复重导。
  if (typeof params.targetUpdatedAt !== "number") {
    return true;
  }
  return params.record.updatedAt > params.targetUpdatedAt;
}
