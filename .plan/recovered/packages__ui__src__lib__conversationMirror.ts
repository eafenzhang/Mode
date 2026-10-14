import {
  CONVERSATION_MIRROR_LIMITS,
  CONVERSATION_MIRROR_VERSION,
  boundConversationMirrorSnapshot,
  type ConversationMirrorIdentity,
  type ConversationMirrorMessage,
  type ConversationMirrorRecord,
  type ConversationMirrorSnapshot,
  type ModeMessageWithParts,
} from "@mode/shared";

/**
 * 远程工作区对话记录镜像的纯逻辑层（见 docs/specs/remote-workspace-conversation-mirror.md）。
 *
 * 这里只做「会话元数据 + 消息 → 快照」和「多份快照 → 展示列表」的投影，不碰任何服务：
 * 读写都由调用方（hook / 组件）通过 conversationMirrorService 完成，便于单测与替换。
 */

/** 参与镜像的最小会话信息；`workspaceIdentity` 用于判定是不是本机本地 scope。 */
export interface ConversationMirrorSourceTask {
  taskId: string;
  title: string;
  createdAt: number;
  updatedAt: number;
  workspaceIdentity?: string;
}

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
    // synthetic / 隐藏消息（分享导入上下文、系统提醒等）不属于「对话记录」，不进镜像。
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

/**
 * 只发布本机本地 scope 产生的会话。
 *
 * 带 `workspaceIdentity` 的行是「以某个对端身份打开的工作区」的会话——它们的原始记录在
 * 对端机器上，再发布一次等于转发对端内容（不变量 2：不转发、不成环）。
 */
export function isMirrorPublishableTask(task: ConversationMirrorSourceTask): boolean {
  return !task.workspaceIdentity?.trim();
}

/** 按 updatedAt 取最新的前 N 条候选（快照本身还有消息数/字节上限兜底）。 */
export function selectMirrorPublishableTasks(
  tasks: readonly ConversationMirrorSourceTask[],
  maxRecords: number = CONVERSATION_MIRROR_LIMITS.maxRecords,
): ConversationMirrorSourceTask[] {
  return tasks
    .filter(isMirrorPublishableTask)
    .slice()
    .sort((left, right) => right.updatedAt - left.updatedAt)
    .slice(0, maxRecords);
}

export interface BuildConversationMirrorSnapshotParams {
  identity: ConversationMirrorIdentity;
  workspacePath: string;
  tasks: readonly ConversationMirrorSourceTask[];
  readMessages: (taskId: string) => Promise<readonly ModeMessageWithParts[]>;
  maxRecords?: number;
  generatedAt?: number;
  /** 单条会话读取失败只跳过该会话，不放弃整份快照。 */
  onReadError?: (taskId: string, error: unknown) => void;
}

/**
 * 构建本机自快照。没有任何可发布会话（或全部读取失败）时返回 null：
 * 不发布空快照，避免用一个「本机没有记录」的空文件覆盖对端已有数据。
 */
export async function buildConversationMirrorSnapshot(
  params: BuildConversationMirrorSnapshotParams,
): Promise<ConversationMirrorSnapshot | null> {
  const candidates = selectMirrorPublishableTasks(params.tasks, params.maxRecords);
  if (candidates.length === 0) {
    return null;
  }

  const records: ConversationMirrorRecord[] = [];
  for (const task of candidates) {
    let messages: ConversationMirrorMessage[];
    try {
      messages = extractConversationMirrorMessages(await params.readMessages(task.taskId));
    } catch (error) {
      params.onReadError?.(task.taskId, error);
      continue;
    }
    if (messages.length === 0) {
      continue;
    }
    records.push({
      sessionId: task.taskId,
      title: task.title,
      createdAt: task.createdAt,
      updatedAt: task.updatedAt,
      messageCount: messages.length,
      truncated: false,
      messages,
    });
  }

  if (records.length === 0) {
    return null;
  }

  return boundConversationMirrorSnapshot({
    version: CONVERSATION_MIRROR_VERSION,
    originDeviceId: params.identity.deviceId,
    originLabel: params.identity.label,
    generatedAt: params.generatedAt ?? Date.now(),
    workspacePath: params.workspacePath,
    records,
  });
}

/** 同一 origin 只保留 generatedAt 较新的一份（远端新读到的覆盖本地缓存）。 */
export function mergeConversationMirrorSnapshots(
  base: readonly ConversationMirrorSnapshot[],
  overlay: readonly ConversationMirrorSnapshot[],
): ConversationMirrorSnapshot[] {
  const byOrigin = new Map<string, ConversationMirrorSnapshot>();
  for (const snapshot of [...base, ...overlay]) {
    const existing = byOrigin.get(snapshot.originDeviceId);
    if (!existing || snapshot.generatedAt >= existing.generatedAt) {
      byOrigin.set(snapshot.originDeviceId, snapshot);
    }
  }
  return [...byOrigin.values()];
}

export interface ConversationMirrorDisplayRecord extends ConversationMirrorRecord {
  originDeviceId: string;
  originLabel: string;
  /** 该会话内容被截断（消息条数 / 文本长度 / 字节预算）。 */
  originTruncated: boolean;
}

export interface ConversationMirrorDisplay {
  records: ConversationMirrorDisplayRecord[];
  peers: Array<{ deviceId: string; label: string; generatedAt: number }>;
}

/**
 * 把多份快照摊平成展示列表：剔除本机自己发布的快照（那是本机真实任务，不该再镜像一份），
 * 按会话 updatedAt 降序。
 */
export function collectConversationMirrorDisplay(
  snapshots: readonly ConversationMirrorSnapshot[],
  selfDeviceId: string | null,
): ConversationMirrorDisplay {
  const peers = new Map<string, { deviceId: string; label: string; generatedAt: number }>();
  const records: ConversationMirrorDisplayRecord[] = [];
  for (const snapshot of snapshots) {
    if (selfDeviceId && snapshot.originDeviceId === selfDeviceId) {
      continue;
    }
    const existing = peers.get(snapshot.originDeviceId);
    if (!existing || snapshot.generatedAt >= existing.generatedAt) {
      peers.set(snapshot.originDeviceId, {
        deviceId: snapshot.originDeviceId,
        label: snapshot.originLabel,
        generatedAt: snapshot.generatedAt,
      });
    }
    for (const record of snapshot.records) {
      records.push({
        ...record,
        originDeviceId: snapshot.originDeviceId,
        originLabel: snapshot.originLabel,
        originTruncated: record.truncated,
      });
    }
  }
  records.sort((left, right) => right.updatedAt - left.updatedAt);
  return { records, peers: [...peers.values()] };
}
