/**
 * 同 workspace 的 bot 回合并入门（纯内存，无 IO）。
 * 多用户并发首发同一 workspace 时 N 个 agent 回合同时起跑造成资源/限流尖峰
 * （docs/specs/bot-workspace-turn-admission.md）。
 */

/** 同一 workspace 内由 bot 侧发起的运行中回合并入上限。 */
export const BOT_WORKSPACE_TURN_CONCURRENCY_CAP = 2;

export interface WorkspaceTurnGate {
  /** 获取槽位；超限时入该 workspace 的 FIFO 等待队列。同 taskId 幂等。 */
  enter(workspaceKey: string, taskId: string): Promise<void>;
  /** 释放槽位；未知/重复 taskId 为 no-op，并唤醒该 workspace 队首等待者。 */
  exit(taskId: string): void;
  /** 清空全部跟踪并唤醒所有等待者（进程收尾/对账用）。 */
  reset(): void;
  count(workspaceKey: string): number;
}

interface Waiter {
  workspaceKey: string;
  resolve: () => void;
}

export function createWorkspaceTurnGate(
  cap: number = BOT_WORKSPACE_TURN_CONCURRENCY_CAP,
): WorkspaceTurnGate {
  const limit = Math.max(1, cap);
  const workspaceByTaskId = new Map<string, string>();
  const waiters: Waiter[] = [];

  const count = (workspaceKey: string): number => {
    let total = 0;
    for (const key of workspaceByTaskId.values()) {
      if (key === workspaceKey) {
        total += 1;
      }
    }
    return total;
  };

  const wakeNext = (workspaceKey: string): void => {
    const index = waiters.findIndex((waiter) => waiter.workspaceKey === workspaceKey);
    if (index < 0) {
      return;
    }
    const [waiter] = waiters.splice(index, 1);
    waiter?.resolve();
  };

  return {
    async enter(workspaceKey: string, taskId: string): Promise<void> {
      if (workspaceByTaskId.has(taskId)) {
        return;
      }
      // 检查与登记之间没有 await：检查→入队→挂起在同一同步段完成，
      // exit 只可能发生在我们挂起之后，不会出现「唤醒时等待者还没入队」的漏唤醒。
      while (count(workspaceKey) >= limit) {
        await new Promise<void>((resolve) => {
          waiters.push({ workspaceKey, resolve });
        });
        // 被唤醒后重新检查计数：槽位可能已被同 workspace 的其他等待者抢走。
        if (workspaceByTaskId.has(taskId)) {
          return;
        }
      }
      workspaceByTaskId.set(taskId, workspaceKey);
    },
    exit(taskId: string): void {
      const workspaceKey = workspaceByTaskId.get(taskId);
      workspaceByTaskId.delete(taskId);
      if (workspaceKey !== undefined) {
        wakeNext(workspaceKey);
      }
    },
    reset(): void {
      workspaceByTaskId.clear();
      const pending = waiters.splice(0, waiters.length);
      for (const waiter of pending) {
        waiter.resolve();
      }
    },
    count,
  };
}
