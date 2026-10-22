/**
 * 远端 workspace 断连后的**有限自动重连**（产品决策：B）。
 *
 * 规则：
 * - 已建立的连接异常关闭 → 最多 3 次尝试，退避 1s / 3s / 9s；
 * - 干净退出（exitCode 0 且无信号/错误）不重试——那是对端主动收工，重连会把它拉回来；
 * - 鉴权/版本/协议类错误不重试——重试只会重复失败，必须让用户处理；
 * - 其余（WS 断开、非零退出、被信号带走、未知错误）重试；
 * - 启动时不自动重连：只在**断连事件**后调度（启动恢复仍只还原断连态 tab）。
 *
 * 本模块只做纯决策与定时调度，不碰 React/平台；调用方负责在手动重连、Bot 重连、
 * tab 关闭或卸载时 `cancel()`。
 */

/** 第 1/2/3 次尝试前的等待时间；次数由长度决定。 */
export const REMOTE_AUTO_RECONNECT_DELAYS_MS = [1_000, 3_000, 9_000] as const;

export type RemoteSessionCloseRetryClassification =
  | { retryable: true; detail: "transport" | "crash" | "unknown-error" }
  | { retryable: false; detail: "clean-exit" | "non-retryable-error" };

/**
 * 鉴权/版本/协议类错误：重试没有意义。
 *
 * 匹配的是**错误文本**而不是结构化错误码：不同后端（LAN WS / SSH / WSL / Docker）
 * 给出的错误没有统一 code，这里只在明确命中时拒绝重试，其余一律按可重试处理——
 * 宁可多试两次，也不要把一次可恢复的断连直接判死。
 */
const NON_RETRYABLE_ERROR_PATTERN =
  /(auth|unauthor|forbidden|permission denied|credential|invalid token|token expired|version|protocol|unsupported|not support|mismatch)/iu;

export function classifyRemoteSessionClose(event: {
  exitCode: number | null;
  signal: string | null;
  error?: string;
}): RemoteSessionCloseRetryClassification {
  const error = event.error?.trim() ?? "";
  if (error) {
    return NON_RETRYABLE_ERROR_PATTERN.test(error)
      ? { retryable: false, detail: "non-retryable-error" }
      : { retryable: true, detail: "unknown-error" };
  }
  if (event.signal !== null) {
    return { retryable: true, detail: "crash" };
  }
  // LAN WS 断开：既没有退出码也没有信号。
  if (event.exitCode === null) {
    return { retryable: true, detail: "transport" };
  }
  // 对端进程主动、干净地退出：不把它拉回来。
  return event.exitCode === 0
    ? { retryable: false, detail: "clean-exit" }
    : { retryable: true, detail: "crash" };
}

export interface RemoteAutoReconnectAttemptResult {
  ok: boolean;
  error?: string;
}

export interface RemoteAutoReconnectSchedulerOptions {
  delaysMs?: readonly number[];
  /**
   * 执行一次重连。返回 `ok: true` 表示成功、或已有别的重连路径接管（让位），
   * 两种情况都停止本组重试。
   */
  attempt: (
    workspaceKey: string,
    attemptIndex: number,
  ) => Promise<RemoteAutoReconnectAttemptResult>;
  onAttemptFailed?: (workspaceKey: string, attemptIndex: number, error: string) => void;
  onExhausted?: (workspaceKey: string, error: string) => void;
  /** 定时器句柄保持不透明：浏览器返回 number、Node 返回 Timeout，测试还会注入替身。 */
  setTimeout?: (handler: () => void, ms: number) => unknown;
  clearTimeout?: (handle: unknown) => void;
}

export interface RemoteAutoReconnectScheduler {
  /** 开始（或重新开始）一个 workspace 的重试链；重复调用会替换旧链。 */
  schedule(workspaceKey: string): void;
  /** 取消该 workspace 的待执行尝试；正在执行的一次不会被中断，但其后续不会再生效。 */
  cancel(workspaceKey: string): void;
  cancelAll(): void;
  isScheduled(workspaceKey: string): boolean;
}

interface ScheduledEntry {
  token: number;
  attemptIndex: number;
  timer: unknown | null;
}

export function createRemoteAutoReconnectScheduler(
  options: RemoteAutoReconnectSchedulerOptions,
): RemoteAutoReconnectScheduler {
  const delays = options.delaysMs?.length ? options.delaysMs : REMOTE_AUTO_RECONNECT_DELAYS_MS;
  const scheduleTimer =
    options.setTimeout ?? ((handler: () => void, ms: number) => setTimeout(handler, ms));
  const clearTimer =
    options.clearTimeout ??
    ((handle: unknown) => clearTimeout(handle as ReturnType<typeof setTimeout>));
  const entries = new Map<string, ScheduledEntry>();
  let nextToken = 1;

  const arm = (workspaceKey: string, entry: ScheduledEntry): void => {
    const delay = delays[Math.min(entry.attemptIndex, delays.length - 1)]!;
    entry.timer = scheduleTimer(() => {
      void fire(workspaceKey, entry.token);
    }, delay);
  };

  const fire = async (workspaceKey: string, token: number): Promise<void> => {
    const entry = entries.get(workspaceKey);
    if (!entry || entry.token !== token) {
      return;
    }
    entry.timer = null;
    const attemptIndex = entry.attemptIndex;
    let result: RemoteAutoReconnectAttemptResult;
    try {
      result = await options.attempt(workspaceKey, attemptIndex);
    } catch (error) {
      result = { ok: false, error: error instanceof Error ? error.message : String(error) };
    }
    // 执行期间被 cancel / 重新 schedule：本次结果作废，也不得再排下一次。
    const current = entries.get(workspaceKey);
    if (!current || current.token !== token) {
      return;
    }
    if (result.ok) {
      entries.delete(workspaceKey);
      return;
    }
    const error = result.error ?? "";
    options.onAttemptFailed?.(workspaceKey, attemptIndex, error);
    if (attemptIndex + 1 < delays.length) {
      entry.attemptIndex = attemptIndex + 1;
      arm(workspaceKey, entry);
      return;
    }
    entries.delete(workspaceKey);
    options.onExhausted?.(workspaceKey, error);
  };

  return {
    schedule(workspaceKey) {
      const existing = entries.get(workspaceKey);
      if (existing?.timer) {
        clearTimer(existing.timer);
      }
      const entry: ScheduledEntry = {
        token: nextToken,
        attemptIndex: 0,
        timer: null,
      };
      nextToken += 1;
      entries.set(workspaceKey, entry);
      arm(workspaceKey, entry);
    },
    cancel(workspaceKey) {
      const entry = entries.get(workspaceKey);
      if (!entry) {
        return;
      }
      if (entry.timer) {
        clearTimer(entry.timer);
      }
      entries.delete(workspaceKey);
    },
    cancelAll() {
      for (const entry of entries.values()) {
        if (entry.timer) {
          clearTimer(entry.timer);
        }
      }
      entries.clear();
    },
    isScheduled(workspaceKey) {
      return entries.has(workspaceKey);
    },
  };
}
