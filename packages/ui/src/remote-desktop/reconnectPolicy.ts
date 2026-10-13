/**
 * 断线重连退避策略（P2-1，纯函数，测试见 remoteDesktopReconnectPolicy.test.ts）。
 *
 * 媒体 WS 瞬断（nativeMediaSession）与整会话重连（useRemoteDesktopSession）共用同一阶梯：
 * 250ms 起、2s 封顶；尝试次数上限 8（≈14s 覆盖一次网络抖动窗口），耗尽按永久失败收口。
 * 越界入参按第 1 次（250ms）处理，步进不放大（attempt 是 1 起计数）。
 */

/** 单侧重连尝试上限（两侧独立计数：媒体级与整会话级各自退避）。 */
export const RECONNECT_MAX_ATTEMPTS = 8;

export function nextReconnectDelayMs(attempt: number): number {
  return Math.min(250 * 2 ** Math.max(0, attempt - 1), 2_000);
}
