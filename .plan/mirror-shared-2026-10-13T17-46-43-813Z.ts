import { z } from "zod";

/**
 * 远程工作区对话记录镜像（见 docs/specs/remote-workspace-conversation-mirror.md）。
 *
 * 语义：同步域是 workspacePath；每台机器把自己在该路径下**本机产生**的会话发布成一份
 * 「origin = 自己」的快照，存在自己的数据目录里。对端连接时既发布自己的快照，也读取对端
 * 的快照。读取方按 originDeviceId 覆盖式幂等去重，并剔除自己的快照（防回声）。
 */

/** 快照文件格式版本。读取方遇到未知版本按「不认识」跳过，不做兼容猜测。 */
export const CONVERSATION_MIRROR_VERSION = 1;

/** 数据根下 `<appConfigDir>/conversation-mirror/<workspacePathHash>/<originDeviceId>.json`。 */
export const CONVERSATION_MIRROR_DIR_NAME = "conversation-mirror";

export const CONVERSATION_MIRROR_LIMITS = {
  /** 单份快照最多会话条数（按 updatedAt 降序保留最新）。 */
  maxRecords: 20,
  /** 单条会话最多消息条数（保留尾部）。 */
  maxMessagesPerSession: 200,
  /** 单条消息最大字符数。 */
  maxTextCharsPerMessage: 4_000,
  /** 单份快照最大字节数（与远端 RPC 分帧量级一致）。 */
  maxSnapshotBytes: 512 * 1024,
} as const;

export interface ConversationMirrorLimits {
  maxRecords: number;
  maxMessagesPerSession: number;
  maxTextCharsPerMessage: number;
  maxSnapshotBytes: number;
}

/** 截断标记：仅追加省略号，不写自然语言（数据会跨语言环境传递）。 */
const TRUNCATION_SUFFIX = "…";

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
    /** 会话真实消息条数；`messages` 可能因上限被截断。 */
    messageCount: z.number().int().nonnegative(),
    /** 本条记录的内容是否被截断（消息条数或文本长度）。 */
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
    reason: z.enum(["stored", "self-origin", "invalid-record"]).optional(),
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

function boundMessageText(text: string, maxChars: number): string {
  if (text.length <= maxChars) {
    return text;
  }
  return `${text.slice(0, maxChars)}${TRUNCATION_SUFFIX}`;
}

function boundConversationMirrorRecord(
  record: ConversationMirrorRecord,
  limits: ConversationMirrorLimits,
): ConversationMirrorRecord {
  const kept = record.messages.slice(-limits.maxMessagesPerSession);
  const messages = kept.map((message) => {
    const text = boundMessageText(message.text, limits.maxTextCharsPerMessage);
    return text === message.text ? message : { ...message, text };
  });
  const textTruncated = messages.some((message, index) => message.text !== kept[index]?.text);
  return {
    ...record,
    messages,
    truncated: record.truncated || textTruncated || kept.length < record.messages.length,
  };
}

const textEncoder = new TextEncoder();

/** UTF-8 字节长度（RPC 分帧按字节计，不能用字符串长度近似）。 */
export function conversationMirrorSnapshotByteSize(snapshot: ConversationMirrorSnapshot): number {
  return textEncoder.encode(JSON.stringify(snapshot)).length;
}

/**
 * 有界化：先按会话数/消息数/文本长度截断，再按字节预算从最旧会话开始丢弃。
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
