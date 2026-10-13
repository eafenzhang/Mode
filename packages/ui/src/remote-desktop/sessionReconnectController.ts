/**
 * 整会话重连控制器（P2-1，从 useRemoteDesktopSession 拆出，max-lines 约束）：
 * 信令传输断开（非 bye/本地结束）后自动重走 `POST /api/remote-desktop/session`
 * （被控对 90s 窗口内同设备重连免二次同意）。退避 250ms 起、2s 封顶、8 次上限
 * （reconnectPolicy）；busy（被控旧会话收口中）与 internal（瞬时）可重试，
 * 明确拒绝（disabled/consent_denied 等）立即回调收口。纯编排，不碰 React 与平台层
 * （startSession 由调用方注入，测试可完全离线驱动）。
 */
import type { RemoteDesktopErrorCode } from "@mode/shared/remote-desktop";
import { nextReconnectDelayMs, RECONNECT_MAX_ATTEMPTS } from "./reconnectPolicy.js";

export interface SessionReconnectController {
  /** 传输断开入口（幂等：退避已在进行中则忽略）。 */
  begin(): void;
  /** 会话卸载/本地结束：停表并作废在途尝试。 */
  dispose(): void;
}

export function createSessionReconnectController(options: {
  /** 一轮重连：即 platform.rdStartSession（成功时携新媒体地址）。 */
  startSession: () => Promise<
    { ok: true; mediaUrl?: string } | { error: RemoteDesktopErrorCode }
  >;
  /** 进入重连的即时处理（调用方：静默停旧媒体面 + 状态「正在恢复」）。 */
  onBegin: () => void;
  /** 重连成功（新 mediaUrl 已拿到，后续连接/媒体事件走正常事件流）。 */
  onRecovered: (mediaUrl: string) => void;
  /** 失败收口（明确拒绝 / 退避耗尽 / 成功但缺 mediaUrl）。 */
  onGiveUp: (error: RemoteDesktopErrorCode) => void;
}): SessionReconnectController {
  let timer: ReturnType<typeof setTimeout> | null = null;
  let attempts = 0;
  let disposed = false;

  const schedule = (delayMs: number): void => {
    timer = setTimeout(() => {
      timer = null;
      void attempt();
    }, delayMs);
  };

  const retryOrGiveUp = (error: RemoteDesktopErrorCode): void => {
    const retryable = error === "busy" || error === "internal";
    if (retryable && attempts < RECONNECT_MAX_ATTEMPTS) {
      schedule(nextReconnectDelayMs(attempts));
      return;
    }
    options.onGiveUp(error);
  };

  const attempt = async (): Promise<void> => {
    if (disposed) {
      return;
    }
    attempts += 1;
    let result: { ok: true; mediaUrl?: string } | { error: RemoteDesktopErrorCode };
    try {
      result = await options.startSession();
    } catch {
      // 桥接异常（main 不可达等）：按 internal 语义走重试/收口。
      retryOrGiveUp("internal");
      return;
    }
    if (disposed) {
      return;
    }
    if ("error" in result) {
      retryOrGiveUp(result.error);
      return;
    }
    if (!result.mediaUrl) {
      // 契约 E 未给出连接地址（旧 main / 未贯通）：与首连同口径按不支持收口。
      options.onGiveUp("unsupported");
      return;
    }
    options.onRecovered(result.mediaUrl);
  };

  return {
    begin() {
      if (disposed || timer !== null) {
        return;
      }
      attempts = 0;
      options.onBegin();
      schedule(nextReconnectDelayMs(0));
    },
    dispose() {
      disposed = true;
      if (timer !== null) {
        clearTimeout(timer);
        timer = null;
      }
    },
  };
}
