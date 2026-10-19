/**
 * Bot 回合全局准入门（纯内存，无 IO）。
 * A+ 之前按 workspace 限制同时运行的 bot 回合数（上限 2），多个群/私聊在同一
 * workspace 内会互相阻塞。A+ 改为单实例全局有界并发：同一会话仍由
 * enqueueInboundProcessing 按对话键串行，不同群/私聊在全局槽位内并行
 * （docs/specs/im-bot-agent-channel-aplus.md、bot-workspace-turn-admission.md）。
 */

/** 单服务实例内同时运行的 bot 回合数上限。 */
export const BOT_GLOBAL_TURN_CONCURRENCY_CAP = 8;

export interface BotTurnGateEnterOptions {
  /**
   * 首次因超限而挂起时同步回调一次（同一 taskId 只回调一次），用于向用户给出
   * 「已排队」提示。契约：同步、不可抛、不阻塞准入；门本身不依赖消息发送。
   */
  onWait?: () => void;
}

export interface BotTurnGate {
  /** 获取全局槽位；超限时进入全局 FIFO 等待队列。同 taskId 幂等。 */
  enter(workspaceKey: string, taskId: string, options?: BotTurnGateEnterOptions): Promise<void>;
  /** 释放槽位；未知/重复 taskId 为 no-op，并唤醒全局队首等待者。 */
  exit(taskId: string): void;
  /** 清空全部跟踪并唤醒所有等待者（进程收尾/对账用）。 */
  reset(): void;
  /** 当前全局运行中的回合数。 */
  count(): number;
  /** 指定 workspace 的运行中回合数，仅用于观测/测试。 */
  count(workspaceKey: string): number;
}

interface Waiter {
  resolve: () => void;
}

export function createBotTurnGate(cap: number = BOT_GLOBAL_TURN_CONCURRENCY_CAP): BotTurnGate {
  const limit = Math.max(1, cap);
  const workspaceByTaskId = new Map<string, string>();
  const waiters: Waiter[] = [];
  /**
   * 在途的 enter 结果（尚未拿到槽位）。重复 enter 同一 taskId 时返回同一个 Promise，
   * 否则会为该 taskId 再压一个 waiter，而 exit 只唤醒其中一个——另一个永远挂着。
   */
  const pendingEnters = new Map<string, Promise<void>>();

  const count = (workspaceKey?: string): number => {
    if (workspaceKey === undefined) {
      return workspaceByTaskId.size;
    }
    let total = 0;
    for (const key of workspaceByTaskId.values()) {
      if (key === workspaceKey) {
        total += 1;
      }
    }
    return total;
  };

  const wakeNext = (): void => {
    const waiter = waiters.shift();
    waiter?.resolve();
  };

  return {
    enter(workspaceKey, taskId, options) {
      if (workspaceByTaskId.has(taskId)) {
        return Promise.resolve();
      }
      const inflight = pendingEnters.get(taskId);
      if (inflight) {
        // 已在等待：复用同一结果（含 onWait 只由首个调用者触发一次）。
        return inflight;
      }
      let waitNotified = false;
      const run = (async () => {
        // 检查与登记之间没有 await：检查→入队→挂起在同一同步段完成，
        // exit 只可能发生在我们挂起之后，不会出现「唤醒时等待者还没入队」的漏唤醒。
        while (workspaceByTaskId.size >= limit) {
          if (!waitNotified) {
            waitNotified = true;
            options?.onWait?.();
          }
          await new Promise<void>((resolve) => {
            waiters.push({ resolve });
          });
          // 被唤醒后重新检查全局计数：槽位可能已被其他等待者抢走。
          if (workspaceByTaskId.has(taskId)) {
            return;
          }
        }
        workspaceByTaskId.set(taskId, workspaceKey);
      })();
      pendingEnters.set(taskId, run);
      const clear = () => {
        pendingEnters.delete(taskId);
      };
      void run.then(clear, clear);
      return run;
    },
    exit(taskId) {
      if (!workspaceByTaskId.has(taskId)) {
        return;
      }
      workspaceByTaskId.delete(taskId);
      wakeNext();
    },
    reset() {
      workspaceByTaskId.clear();
      const pending = waiters.splice(0, waiters.length);
      for (const waiter of pending) {
        waiter.resolve();
      }
    },
    count,
  };
}
