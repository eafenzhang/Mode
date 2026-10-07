import type { Locale } from "@zcode/shared";
import { formatGroupTimestamp } from "./groupPrompt.js";

/**
 * 群聊历史缓冲：未被 @ / 未被处理的群消息按会话暂存，作为下一次触发时的上下文。
 * 群聊里机器人不该回应每条消息，但完全失忆会让"刚说过的事"接不上；
 * 这里用一个带上限的内存缓冲折中（与 MyAgents 的 GroupHistoryBuffer 同一思路）。
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

export interface GroupHistoryBuffer {
  append(botId: string, chatId: string, entry: Omit<GroupHistoryEntry, "at">): void;
  drain(botId: string, chatId: string): GroupHistoryEntry[];
  clear(botId: string, chatId: string): void;
  clearBot(botId: string): void;
  size(): number;
}

export function createGroupHistoryBuffer(): GroupHistoryBuffer {
  const groups = new Map<string, GroupHistoryEntry[]>();

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
      while (entries.length > GROUP_HISTORY_MAX_PER_GROUP) {
        entries.shift();
      }
    },
    drain(botId, chatId) {
      const key = groupKey(botId, chatId);
      const entries = groups.get(key) ?? [];
      groups.delete(key);
      return entries;
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
