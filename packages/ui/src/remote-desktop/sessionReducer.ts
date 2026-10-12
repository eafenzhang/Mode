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

export type RemoteDesktopMediaStatus = "idle" | "negotiating" | "live" | "failed";

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
  /** 已收到 media-open（原生媒体面信令已到，下一步连 media WS；用于连接中提示）。 */
  mediaOpenReceived: boolean;
  /** 对端画面冻结（锁屏 / 失去访问权限）：freeze/unfreeze 写入（Task 13 落分支与提示）。 */
  frozen: boolean;
}

export const INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE: RemoteDesktopSessionViewState = {
  phase: "connecting",
  closeKind: null,
  errorCode: null,
  mediaStatus: "idle",
  monitors: [],
  selectedDisplayId: null,
  mediaOpenReceived: false,
  frozen: false,
};

export type RemoteDesktopSessionEffect =
  | { kind: "signal"; message: RemoteDesktopSignalMessage }
  /**
   * 连原生媒体面（契约 D/B）：media-open 携带逐会话 key/salt，URL 由 rdStartSession 直达（契约 E）。
   */
  | { kind: "open-media"; key: string; salt: string }
  // WebRTC 残留 effect（Task 14 随 schema 删除）：状态机已不再产出，仅为 hook 的 no-op 分支保留类型。
  | { kind: "apply-offer"; sdp: string }
  | {
      kind: "add-ice-candidate";
      candidate: { candidate: string; sdpMid: string | null; sdpMLineIndex: number | null };
    }
  | { kind: "end-session"; reason: "user-end" | "error" };

export type RemoteDesktopSessionEvent =
  | { type: "controller"; event: RemoteDesktopControllerEvent }
  | { type: "select-monitor"; displayId: string }
  | { type: "local-end" }
  | { type: "media"; status: RemoteDesktopMediaStatus };

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
  // connecting：start-session 重试时重置上一轮归因。
  return transition(state, {
    phase: "connecting",
    closeKind: null,
    errorCode: null,
    mediaStatus: "idle",
    mediaOpenReceived: false,
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
      const shouldSelect = !keepSelected && selectedDisplayId !== null;
      return transition(
        state,
        { monitors: message.monitors, selectedDisplayId },
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
    case "offer":
    case "ice":
      // WebRTC 残留分支（Task 14 随 schema 删除）：记录性忽略，绝不再驱动 WebRTC effect。
      return transition(state, {});
    case "error":
      return transition(state, { closeKind: "error", errorCode: message.code });
    case "bye":
      if (state.phase === "closed") {
        return transition(state, {});
      }
      return transition(state, { phase: "closed", closeKind: "remote" });
    case "answer":
    case "hello":
    case "select-monitor":
      // 主控方向不应收到这些信号（被控端才处理它们）：记录性忽略，不改状态。
      return transition(state, {});
    case "freeze":
    case "unfreeze":
      // 冻结/解冻信令（被控 → 主控）：frozen 状态字段已就位，消费分支 Task 13 落；
      // 显式列出是为保持 switch 对 union 的穷举（否则 TS2366 缺结尾 return）。
      return transition(state, {});
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
      return transition(state, { selectedDisplayId: event.displayId }, [
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
  }
}
