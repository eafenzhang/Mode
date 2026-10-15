import type { Locale } from "@mode/shared";
import { formatGroupTimestamp } from "./groupPrompt.js";

/**
 * 群聊历史缓冲：未被 @ / 未被处理的群消息按会话暂存，作为下一次触发时的上下文。
 * 群聊里机器人不该回应每条消息，但完全失忆会让"刚说过的事"接不上；
 * 这里用一个带上限的缓冲折中（与 MyAgents 的 GroupHistoryBuffer 同一思路）。
 *
 * 读取语义是**快照**而非取走：被触发一次之后历史仍在，同一群下次触发还能看到
 * （旧 `drain()` 的"取走即清空"已废止）。落盘由 `groupHistoryStore` 负责——
 * 历史不进 `bot-state`，避免高频入站触发全量状态写盘
 * （docs/specs/im-bot-group-chat-alignment.md「G7 实现口径」）。
 */

export interface GroupHistoryEntry {
  senderName?: string;
  text: string;
  at: number;
}

export const GROUP_HISTORY_MAX_PER_GROUP = 30;
export const GROUP_HISTORY_MAX_GROUPS = 200;
/** 单条历史截断长度：上下文价值有限，避免把超长消息整段带进 prompt。 */
const GROUP_HISTORY_ENTRY_MAX_CHARS = 200;

function groupKey(botId: string, chatId: string): string {
  return `${botId}\u0000${chatId}`;
}

/** 落盘形态：{ 版本, bots: { [botId]: { [chatId]: entries } } }。 */
export interface GroupHistorySnapshot {
  version: 1;
  bots: Record<string, Record<string, GroupHistoryEntry[]>>;
}

export interface GroupHistoryBuffer {
  append(botId: string, chatId: string, entry: Omit<GroupHistoryEntry, "at">): void;
  /** 读取该群历史但**不清空**（触发回合后仍保留）。 */
  snapshot(botId: string, chatId: string): GroupHistoryEntry[];
  clear(botId: string, chatId: string): void;
  clearBot(botId: string): void;
  size(): number;
  /** 从落盘快照恢复；幂等（已恢复过则忽略），非法内容按空处理。 */
  hydrate(snapshot: GroupHistorySnapshot | null | undefined): void;
  /** 导出全部历史用于落盘。 */
  serialize(): GroupHistorySnapshot;
}

/** 归一化单条历史：非法条目丢弃，超长截断。 */
function normalizeEntry(value: unknown): GroupHistoryEntry | null {
  if (typeof value !== "object" || value === null) {
    return null;
  }
  const raw = value as Partial<GroupHistoryEntry>;
  if (typeof raw.text !== "string" || typeof raw.at !== "number") {
    return null;
  }
  const text = raw.text.trim();
  if (!text) {
    return null;
  }
  return {
    ...(typeof raw.senderName === "string" && raw.senderName.trim()
      ? { senderName: raw.senderName.trim() }
      : {}),
    text:
      text.length > GROUP_HISTORY_ENTRY_MAX_CHARS
        ? `${text.slice(0, GROUP_HISTORY_ENTRY_MAX_CHARS)}…`
        : text,
    at: raw.at,
  };
}

export function createGroupHistoryBuffer(): GroupHistoryBuffer {
  const groups = new Map<string, GroupHistoryEntry[]>();
  let hydrated = false;

  const touch = (key: string): GroupHistoryEntry[] => {
    const existing = groups.get(key);
    if (existing) {
      // 重新 set 让该键回到迭代序末尾（Map 保持插入序，可当 LRU 用）。
      groups.delete(key);
      groups.set(key, existing);
      return existing;
    }
    const created: GroupHistoryEntry[] = [];
    groups.set(key, created);
    while (groups.size > GROUP_HISTORY_MAX_GROUPS) {
      const oldest = groups.keys().next().value;
      if (oldest === undefined) {
        break;
      }
      groups.delete(oldest);
    }
    return created;
  };

  const trimGroup = (entries: GroupHistoryEntry[]): void => {
    while (entries.length > GROUP_HISTORY_MAX_PER_GROUP) {
      entries.shift();
    }
  };

  return {
    append(botId, chatId, entry) {
      const text = entry.text.trim();
      if (!text) {
        return;
      }
      const entries = touch(groupKey(botId, chatId));
      entries.push({
        ...(entry.senderName?.trim() ? { senderName: entry.senderName.trim() } : {}),
        text:
          text.length > GROUP_HISTORY_ENTRY_MAX_CHARS
            ? `${text.slice(0, GROUP_HISTORY_ENTRY_MAX_CHARS)}…`
            : text,
        at: Date.now(),
      });
      trimGroup(entries);
    },
    snapshot(botId, chatId) {
      return groups.get(groupKey(botId, chatId)) ?? [];
    },
    clear(botId, chatId) {
      groups.delete(groupKey(botId, chatId));
    },
    clearBot(botId) {
      const prefix = `${botId}\u0000`;
      // 先快照键：遍历中删除 Map 条目会跳过后续项。
      for (const key of groups.keys().toArray()) {
        if (key.startsWith(prefix)) {
          groups.delete(key);
        }
      }
    },
    size() {
      return groups.size;
    },
    hydrate(snapshot) {
      if (hydrated) {
        return;
      }
      hydrated = true;
      if (!snapshot || snapshot.version !== 1) {
        return;
      }
      for (const [botId, byChat] of Object.entries(snapshot.bots ?? {})) {
        for (const [chatId, entries] of Object.entries(byChat ?? {})) {
          const normalized = (Array.isArray(entries) ? entries : [])
            .map(normalizeEntry)
            .filter((item): item is GroupHistoryEntry => item !== null);
          if (normalized.length === 0) {
            continue;
          }
          trimGroup(normalized);
          groups.set(groupKey(botId, chatId), normalized);
          while (groups.size > GROUP_HISTORY_MAX_GROUPS) {
            const oldest = groups.keys().next().value;
            if (oldest === undefined) {
              break;
            }
            groups.delete(oldest);
          }
        }
      }
    },
    serialize() {
      const bots: Record<string, Record<string, GroupHistoryEntry[]>> = {};
      for (const [key, entries] of groups.entries()) {
        const separator = key.indexOf("\u0000");
        if (separator < 0) {
          continue;
        }
        const botId = key.slice(0, separator);
        const chatId = key.slice(separator + 1);
        (bots[botId] ??= {})[chatId] = [...entries];
      }
      return { version: 1, bots };
    },
  };
}

/** 渲染为提示词上下文块；无历史返回 null。 */
export function formatGroupHistoryContext(
  entries: readonly GroupHistoryEntry[],
  locale: Locale | undefined,
): string | null {
  if (entries.length === 0) {
    return null;
  }
  const lines = entries.map(entry => {
    // 行格式与群聊消息标记一致（[from: 名字 时间]）：提示词里"历史"和"当前消息"同构，
    // 模型不必在两种格式间切换。
    const sender =
      entry.senderName?.trim() || (locale === "en-US" ? "unknown member" : "未知成员");
    return `[from: ${sender} ${formatGroupTimestamp(entry.at)}] ${entry.text}`;
  });
  const header =
    locale === "en-US"
      ? "[Recent group chat history for context only — the user has not replied to this yet]"
      : "[以下是机器人未参与期间的群聊记录，仅供参考]";
  const footer =
    locale === "en-US"
      ? "[End of group chat history. Now answer the current message below.]"
      : "[以上为群聊记录。接下来请回应下面的当前消息。]";
  return [header, ...lines, footer].join("\n");
}
