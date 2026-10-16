/**
 * 镜像来源读取失败的退避表（docs/specs/remote-workspace-conversation-mirror.md「第四轮修复」成因 C）。
 *
 * 有些来源会话会**稳定地**读不出来（例如 wire 校验拒收的 part 形态）。镜像原来对每条来源都是
 * 「本轮读失败 → 下一轮接着读」，于是同一条会话被无限重试，每一轮都打印一次同样的错误；
 * 叠加「空转轮也重读全部来源」之后，同步就再也没有收敛的时候。
 *
 * 这里把「失败过的来源」按 taskId 记一段冷静期：期内不重读，但**仍然计失败信号**（调用方据此报 degraded），
 * 所以连续失败既不会每轮重撞，也不会被退避洗成「一切正常」。成功一次即清零。
 *
 * 状态所有权：一次挂载一个实例，存活在 `useWorkspaceConversationMirror` 的 ref 里，不进 store、
 * 不跨窗口共享——重挂载（重新展开工作区）后重新尝试一次是期望行为。
 */
export interface ConversationMirrorReadBackoff {
  /**
   * 是否应跳过这条来源的读取。返回 `undefined` 表示可以读；返回字符串表示跳过并附带上次失败原因
   * （调用方用它填状态提示，避免「退避期内的轮次」丢失诊断文案）。
   */
  shouldSkip(taskId: string): string | undefined;
  /** 记一次读取失败：冷静期按 失败次数 指数增长。 */
  recordFailure(taskId: string, error: string): void;
  /** 记一次读取成功：该来源恢复正常节奏。 */
  recordSuccess(taskId: string): void;
  /** 清空所有退避（用户手动重试时必须先清，否则「重试」会被自己的退避挡掉）。 */
  reset(): void;
}

/** 首次失败的冷静期。 */
export const CONVERSATION_MIRROR_READ_BACKOFF_BASE_MS = 60_000;
/** 冷静期上限：持续失败退化成低频重试，而不是不重试。 */
export const CONVERSATION_MIRROR_READ_BACKOFF_MAX_MS = 600_000;
/** 跟踪上限：来源列表是有界的，这里只防「来源 id 无限增长」的病态情况。 */
const MAX_TRACKED_TASKS = 200;

interface FailureEntry {
  failures: number;
  skipUntil: number;
  lastError: string;
}

export function createConversationMirrorReadBackoff(params?: {
  baseMs?: number;
  maxMs?: number;
  now?: () => number;
}): ConversationMirrorReadBackoff {
  const baseMs = params?.baseMs ?? CONVERSATION_MIRROR_READ_BACKOFF_BASE_MS;
  const maxMs = params?.maxMs ?? CONVERSATION_MIRROR_READ_BACKOFF_MAX_MS;
  const now = params?.now ?? (() => Date.now());
  const entries = new Map<string, FailureEntry>();

  return {
    shouldSkip(taskId) {
      const entry = entries.get(taskId);
      if (!entry) {
        return undefined;
      }
      return now() < entry.skipUntil ? entry.lastError : undefined;
    },
    recordFailure(taskId, error) {
      const previous = entries.get(taskId);
      const failures = (previous?.failures ?? 0) + 1;
      // 按失败次数指数退避：60s、120s、240s……封顶 CONVERSATION_MIRROR_READ_BACKOFF_MAX_MS。
      const delay = Math.min(baseMs * 2 ** (failures - 1), maxMs);
      entries.delete(taskId);
      entries.set(taskId, { failures, skipUntil: now() + delay, lastError: error });
      while (entries.size > MAX_TRACKED_TASKS) {
        const oldest = entries.keys().next();
        if (oldest.done) {
          break;
        }
        entries.delete(oldest.value);
      }
    },
    recordSuccess(taskId) {
      entries.delete(taskId);
    },
    reset() {
      entries.clear();
    },
  };
}
