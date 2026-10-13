/**
 * 主控会话视图状态机（纯逻辑，单测见 packages/ui/test/remoteDesktopSessionReducer.test.ts）。
 *
 * 输入来自两处：main 推送的 `rd/session-event`（信令通道生命周期 + 对端信令）与页面本地动作
 * （选显示器、断开、媒体状态）。副作用全部以 effect 声明式返回，由 useRemoteDesktopSession
 * 解释执行——状态机本身不碰原生媒体会话、WS 与平台调用。
 *
 * 握手顺序（与 main controllerSession / 被控采集侧一致，改序须同改两边）：
 *   WS connected → 本侧发 hello{protocolVersion} → 对端 monitors → 本侧 select-monitor
 *   → 对端 media-open{key,salt} → 本侧连 media WS（开流；任一端 bye 即结束）。
 *
 * 稳定失败语义（spec 降级与失败语义）：拒绝/超时/busy/令牌失效都由对端错误码归因，
 * 不自动重试；首个结束原因生效，重复/迟到的 closed 不改写理由。
 */
import type { RemoteDesktopErrorCode } from "@mode/shared/remote-desktop";
import {
  REMOTE_DESKTOP_PROTOCOL_VERSION,
  type RemoteDesktopMonitor,
  type RemoteDesktopSignalMessage,
} from "@mode/shared/remote-desktop";
import type { RemoteDesktopControllerEvent } from "./platformTypes.js";

export type RemoteDesktopSessionPhase = "connecting" | "connected" | "closed";

/** 结束归因：user = 本端主动断开；remote = 对端 bye / 通道被关闭；error = 错误码或媒体失败。 */
export type RemoteDesktopSessionCloseKind = "user" | "remote" | "error";

export type RemoteDesktopMediaStatus =
  | "idle"
  | "negotiating"
  | "live"
  | "failed"
  /** 媒体 WS 瞬断/整会话传输断开后的自动重连退避中（P2-1，画面中断提示）。 */
  | "reconnecting";

export interface RemoteDesktopSessionViewState {
  phase: RemoteDesktopSessionPhase;
  closeKind: RemoteDesktopSessionCloseKind | null;
  /** 稳定错误码（对端 session 请求失败 / 对端 error 信令）；媒体失败无码为 null。 */
  errorCode: RemoteDesktopErrorCode | null;
  mediaStatus: RemoteDesktopMediaStatus;
  /** 对端枚举的显示器列表（monitors 信令填充；工具条据此切换）。 */
  monitors: RemoteDesktopMonitor[];
  /** 已选择的显示器（首帧起为对端主屏；切换只发信令，不重建会话）。 */
  selectedDisplayId: string | null;
  /**
   * 本会话是否已发过 select-monitor（会话级欠账，2026-10-13 修复）：media-open 的
   * key/salt 逐会话生成、只随首个 select-monitor 应答下发——会话重建（改档/断线全重建）
   * 后即便显示器未变也必须重发，否则新会话媒体面永不建立（画面冻结在旧帧）。
   * connecting 迁移清零；发出 select-monitor（自动或用户切换）置位。
   */
  selectSentForSession: boolean;
  /** 已收到 media-open（原生媒体面信令已到，下一步连 media WS；用于连接中提示）。 */
  mediaOpenReceived: boolean;
  /** 对端画面冻结（锁屏 / 失去访问权限）：freeze/unfreeze 写入；reconnecting 重连时清除。 */
  frozen: boolean;
}

export const INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE: RemoteDesktopSessionViewState = {
  phase: "connecting",
  closeKind: null,
  errorCode: null,
  mediaStatus: "idle",
  monitors: [],
  selectedDisplayId: null,
  selectSentForSession: false,
  mediaOpenReceived: false,
  frozen: false,
};

export type RemoteDesktopSessionEffect =
  | { kind: "signal"; message: RemoteDesktopSignalMessage }
  /**
   * 连原生媒体面（契约 D/B）：media-open 携带逐会话 key/salt，URL 由 rdStartSession 直达（契约 E）。
   */
  | { kind: "open-media"; key: string; salt: string }
  | { kind: "end-session"; reason: "user-end" | "error" };

export type RemoteDesktopSessionEvent =
  | { type: "controller"; event: RemoteDesktopControllerEvent }
  | { type: "select-monitor"; displayId: string }
  | { type: "local-end" }
  | { type: "media"; status: RemoteDesktopMediaStatus }
  // 本地媒体面控制事件（Task 9 胶水派发 rd-media-control）：与对端 freeze/unfreeze 信令
  // 共用同一转移——信令走 main 转发，媒体帧内控制走 renderer 直派，两条入口一个状态。
  | { type: "freeze" }
  | { type: "unfreeze" };

export interface RemoteDesktopSessionTransition {
  state: RemoteDesktopSessionViewState;
  effects: RemoteDesktopSessionEffect[];
}

function transition(
  state: RemoteDesktopSessionViewState,
  next: Partial<RemoteDesktopSessionViewState>,
  effects: RemoteDesktopSessionEffect[] = [],
): RemoteDesktopSessionTransition {
  return { state: { ...state, ...next }, effects };
}

function pickInitialMonitor(monitors: RemoteDesktopMonitor[]): string | null {
  if (monitors.length === 0) {
    return null;
  }
  const primary = monitors.find((monitor) => monitor.primary);
  return primary ? primary.displayId : (monitors[0]?.displayId ?? null);
}

/**
 * freeze/unfreeze 的唯一转移，两处入口共用：对端信令（main 转发）与媒体面控制事件
 * （rd-media-control，同帧通道直达）。reason（locked / access-lost）已在信令层归一，
 * UI 只需布尔，不进 state。
 */
function applyFrozen(
  state: RemoteDesktopSessionViewState,
  frozen: boolean,
): RemoteDesktopSessionTransition {
  return transition(state, { frozen });
}

function applyControllerState(
  state: RemoteDesktopSessionViewState,
  event: Extract<RemoteDesktopControllerEvent, { kind: "state" }>,
): RemoteDesktopSessionTransition {
  if (event.state === "closed") {
    if (state.phase === "closed") {
      // 首个结束原因生效：本地断开后的迟到 closed 不改写归因。
      return transition(state, {});
    }
    return transition(state, {
      phase: "closed",
      closeKind: event.error ? "error" : "remote",
      errorCode: event.error ?? null,
    });
  }
  if (event.state === "connected") {
    // WS 就绪后由本侧发起 hello（被控端据此回 monitors），失败由对端 error 归因。
    return transition(state, { phase: "connected", closeKind: null, errorCode: null }, [
      {
        kind: "signal",
        message: { type: "hello", protocolVersion: REMOTE_DESKTOP_PROTOCOL_VERSION },
      },
    ]);
  }
  // connecting：start-session 重试时重置上一轮归因；frozen 一并清除——新会话的冻结状态
  // 只能由新一轮 freeze 信令写入，不能继承上一轮（Resolution：与 mediaStatus 同列重置）。
  // selectSentForSession 同列清零：media-open 逐会话生成，新会话必须重发 select-monitor
  // 才能拿到本会话 key/salt（改档/断线全重建 2026-10-13 实机缺陷）。
  return transition(state, {
    phase: "connecting",
    closeKind: null,
    errorCode: null,
    mediaStatus: "idle",
    mediaOpenReceived: false,
    selectSentForSession: false,
    frozen: false,
  });
}

function applyControllerSignal(
  state: RemoteDesktopSessionViewState,
  message: RemoteDesktopSignalMessage,
): RemoteDesktopSessionTransition {
  switch (message.type) {
    case "monitors": {
      if (message.monitors.length === 0) {
        return transition(state, {});
      }
      const keepSelected =
        state.selectedDisplayId !== null &&
        message.monitors.some((monitor) => monitor.displayId === state.selectedDisplayId);
      const selectedDisplayId = keepSelected
        ? state.selectedDisplayId
        : pickInitialMonitor(message.monitors);
      // 会话级欠账：本会话还没发过 select-monitor（首连、或重建后 connecting 已清零）时
      // 必须发——media-open 只随首个 select-monitor 应答下发，漏发即新会话媒体面永不建立。
      // 已选显示器从列表消失（keepSelected=false）同样要重发（对齐旧语义：改选须通知对端）；
      // 仅同会话内重复 monitors 且选择未变时不重发。
      const shouldSelect =
        selectedDisplayId !== null && (!state.selectSentForSession || !keepSelected);
      return transition(
        state,
        { monitors: message.monitors, selectedDisplayId, ...(shouldSelect ? { selectSentForSession: true } : {}) },
        shouldSelect
          ? [{ kind: "signal", message: { type: "select-monitor", displayId: selectedDisplayId } }]
          : [],
      );
    }
    case "media-open":
      // 同意通过：对端下发媒体面 key/salt（契约 D）。连接 media WS 由 open-media effect 完成，
      // URL（mediaTicket）不经信令，由 rdStartSession 返回值直达（契约 E）。
      return transition(state, { mediaOpenReceived: true, mediaStatus: "negotiating" }, [
        { kind: "open-media", key: message.key, salt: message.salt },
      ]);
    case "error":
      return transition(state, { closeKind: "error", errorCode: message.code });
    case "bye":
      if (state.phase === "closed") {
        return transition(state, {});
      }
      return transition(state, { phase: "closed", closeKind: "remote" });
    case "hello":
    case "select-monitor":
      // 主控方向不应收到这些信号（被控端才处理它们）：记录性忽略，不改状态。
      return transition(state, {});
    case "request-media-ticket":
    case "media-ticket":
      // 断线重连控制面（P2-1）：media-ticket 由会话胶水在 dispatch 前拦截并唤醒等票方，
      // 正常不会到达 reducer；到达即防御性忽略（不改视图状态）。
      return transition(state, {});
    case "freeze":
      // 被控端锁屏 / 失去访问权限：画面已冻结，UI 在状态条显示冻结提示（文案不区分 reason）。
      return applyFrozen(state, true);
    case "unfreeze":
      return applyFrozen(state, false);
  }
}

export function reduceRemoteDesktopSession(
  state: RemoteDesktopSessionViewState,
  event: RemoteDesktopSessionEvent,
): RemoteDesktopSessionTransition {
  switch (event.type) {
    case "controller":
      return event.event.kind === "state"
        ? applyControllerState(state, event.event)
        : applyControllerSignal(state, event.event.message);
    case "select-monitor": {
      if (!state.monitors.some((monitor) => monitor.displayId === event.displayId)) {
        return transition(state, {});
      }
      if (state.selectedDisplayId === event.displayId) {
        return transition(state, {});
      }
      return transition(state, { selectedDisplayId: event.displayId, selectSentForSession: true }, [
        { kind: "signal", message: { type: "select-monitor", displayId: event.displayId } },
      ]);
    }
    case "local-end": {
      if (state.phase === "closed") {
        return transition(state, {});
      }
      return transition(state, { phase: "closed", closeKind: "user", errorCode: null }, [
        { kind: "end-session", reason: "user-end" },
      ]);
    }
    case "media": {
      if (event.status === "failed") {
        if (state.phase === "closed") {
          return transition(state, { mediaStatus: "failed" });
        }
        // 媒体面建立失败（无 canvas/mediaUrl、传输断开、解密连续失败）：终止会话并归因，不留半开状态。
        return transition(state, { mediaStatus: "failed", phase: "closed", closeKind: "error" }, [
          { kind: "end-session", reason: "error" },
        ]);
      }
      return transition(state, { mediaStatus: event.status });
    }
    // 媒体面控制事件（rd-media-control）与对端信令共用转移：谁先到都写同一 frozen 布尔。
    case "freeze":
      return applyFrozen(state, true);
    case "unfreeze":
      return applyFrozen(state, false);
  }
}
