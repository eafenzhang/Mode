import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { atomicWritePrivateTextFile, withFileLock } from "@mode/shared/node";
import { getAppConfigDir } from "../paths.js";
import type { GroupHistoryEntry, GroupHistorySnapshot } from "./groupHistory.js";

/**
 * 群历史的落盘（docs/specs/im-bot-group-chat-alignment.md「G7 实现口径」）。
 *
 * 为什么单独一个文件、而不是塞进 `bot-state`：群消息是高频入站，而 `bot-state` 的每次写入
 * 都是锁内全量 RMW + 整文件重写，且该文件多窗口共享——历史挂进去等于让每条未被 @ 的群消息
 * 都触发一次全量状态写盘。
 *
 * 为什么可以去抖：历史只是"机器人未参与期间的参考上下文"，崩溃最多丢最近一次去抖窗口内的
 * 追加，这是本 spec 明确接受的取舍。
 */

export const BOT_GROUP_HISTORY_FILE = "bot-group-history.v1.json";
export const BOT_GROUP_HISTORY_FLUSH_DEBOUNCE_MS = 2_000;

export interface GroupHistoryPersistence {
  /** 读回落盘快照；缺失/损坏都返回 null（历史不值得让启动失败）。 */
  load(): Promise<GroupHistorySnapshot | null>;
  /** 排队落盘（去抖）；同一进程内只保留最新一份快照。 */
  schedule(snapshot: GroupHistorySnapshot): void;
  /** 立即落盘（终态/退出用）；无待写内容时是 no-op。 */
  flush(): Promise<void>;
  dispose(): Promise<void>;
}

function isEntry(value: unknown): value is GroupHistoryEntry {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const entry = value as Partial<GroupHistoryEntry>;
  return typeof entry.text === "string" && typeof entry.at === "number";
}

/** 结构守卫：只认我们自己写出的形状，其余（含人工改坏的文件）一律当没有历史。 */
function isSnapshot(value: unknown): value is GroupHistorySnapshot {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const snapshot = value as Partial<GroupHistorySnapshot>;
  if (snapshot.version !== 1 || typeof snapshot.bots !== "object" || snapshot.bots === null) {
    return false;
  }
  return Object.values(snapshot.bots).every(
    (byChat) =>
      typeof byChat === "object" &&
      byChat !== null &&
      Object.values(byChat).every(
        (entries) => Array.isArray(entries) && entries.every(isEntry),
      ),
  );
}

export function createGroupHistoryPersistence(
  options: { debounceMs?: number; filePath?: string } = {},
): GroupHistoryPersistence {
  const debounceMs = options.debounceMs ?? BOT_GROUP_HISTORY_FLUSH_DEBOUNCE_MS;
  const filePath = options.filePath ?? join(getAppConfigDir(), BOT_GROUP_HISTORY_FILE);
  let timer: ReturnType<typeof setTimeout> | undefined;
  let pending: GroupHistorySnapshot | undefined;
  // 写盘串行链：去抖定时器与显式 flush 可能同时触发，必须避免并发写同一文件。
  let writing: Promise<void> = Promise.resolve();

  const writeNow = async (): Promise<void> => {
    const snapshot = pending;
    pending = undefined;
    if (!snapshot) {
      return;
    }
    await withFileLock(filePath, () =>
      atomicWritePrivateTextFile(filePath, `${JSON.stringify(snapshot)}\n`),
    );
  };

  const cancelTimer = (): void => {
    if (timer) {
      clearTimeout(timer);
      timer = undefined;
    }
  };

  return {
    async load() {
      try {
        const raw: unknown = JSON.parse(await readFile(filePath, "utf8"));
        return isSnapshot(raw) ? raw : null;
      } catch {
        return null;
      }
    },
    schedule(snapshot) {
      pending = snapshot;
      if (timer) {
        return;
      }
      timer = setTimeout(() => {
        timer = undefined;
        writing = writing.catch(() => undefined).then(writeNow).catch(() => undefined);
      }, debounceMs);
      // 不要因为一个待写的历史快照把进程/事件循环吊住。
      timer.unref();
    },
    async flush() {
      cancelTimer();
      writing = writing.catch(() => undefined).then(writeNow);
      await writing;
    },
    async dispose() {
      await this.flush();
    },
  };
}
