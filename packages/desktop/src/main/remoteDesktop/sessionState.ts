/**
 * 被控远程桌面会话状态机（纯逻辑，单测见 packages/desktop/tests/fixtures/remote-desktop-session.ts）。
 *
 * 唯一所有者 = 被控 main（docs/specs/lan-remote-desktop.md 状态所有者表）。
 * 相位单向推进，失败即收口：
 *
 *   idle ──request(未授权)──► requesting ──consent-shown──► waitingConsent ──allow──► active ──close──► closed
 *     └────request(已授权)──────────────────────────────────────────────────────────► active ──close──► closed
 *   closed ──cleanup-done──► idle（下一会话重新开始）
 *
 * 单会话互斥：非 idle 相位收到 request → 拒绝 busy（不迁移、不弹第二个同意窗）。
 * 副作用全部以动作（actions）声明式返回，由 coordinator 解释执行——状态机本身不碰窗口/网络。
 */
import type { RemoteDesktopErrorCode } from "@mode/shared/remote-desktop";

export type RemoteDesktopServerPhase = "idle" | "requesting" | "waitingConsent" | "active" | "closed";

/** 会话收口原因（单出口 closeSession(reason) 使用；写 info 日志）。 */
export type RemoteDesktopSessionCloseReason =
  | "consent-denied"
  | "peer-end"
  | "local-end"
  | "socket-closed"
  | "error"
  | "app-quit"
  | "disabled";

export interface RemoteDesktopSessionState {
  phase: RemoteDesktopServerPhase;
  clientId: string | null;
  label: string | null;
  closeReason: RemoteDesktopSessionCloseReason | null;
}

export const INITIAL_REMOTE_DESKTOP_SESSION_STATE: RemoteDesktopSessionState = {
  phase: "idle",
  clientId: null,
  label: null,
  closeReason: null,
};

export type RemoteDesktopSessionEvent =
  | { type: "request"; clientId: string; label: string; authorized: boolean }
  | { type: "consent-shown" }
  | { type: "consent-decided"; outcome: "allow" | "deny" | "timeout"; remember: boolean }
  | { type: "close"; reason: RemoteDesktopSessionCloseReason }
  | { type: "cleanup-done" };

export type RemoteDesktopSessionAction =
  | { kind: "open-consent"; clientId: string; label: string }
  | { kind: "close-consent" }
  | { kind: "answer-consent"; outcome: "allow" | "deny" | "timeout"; remember: boolean }
  | { kind: "start-indicator"; clientId: string; label: string }
  | { kind: "stop-indicator" }
  | { kind: "start-capture" }
  | { kind: "stop-capture" };

export interface RemoteDesktopSessionTransition {
  state: RemoteDesktopSessionState;
  actions: RemoteDesktopSessionAction[];
  /** request 被拒时的稳定错误码（busy / disabled）；其余事件不产生。 */
  rejected?: RemoteDesktopErrorCode;
}

export function isRemoteDesktopSessionBusy(state: RemoteDesktopSessionState): boolean {
  return state.phase !== "idle";
}

function withConsentActions(
  clientId: string,
  label: string,
  outcome: "allow" | "deny" | "timeout",
  remember: boolean,
): RemoteDesktopSessionAction[] {
  const actions: RemoteDesktopSessionAction[] = [
    { kind: "close-consent" },
    { kind: "answer-consent", outcome, remember },
  ];
  if (outcome === "allow") {
    actions.push({ kind: "start-indicator", clientId, label }, { kind: "start-capture" });
  }
  return actions;
}

function enterActive(
  state: RemoteDesktopSessionState,
  actions: RemoteDesktopSessionAction[],
): RemoteDesktopSessionTransition {
  return {
    state: {
      phase: "active",
      clientId: state.clientId,
      label: state.label,
      closeReason: null,
    },
    actions,
  };
}

/**
 * 状态迁移（纯函数）。未定义组合一律「保持原状态、无动作」——重复/迟到的消息不得让会话回退。
 * consent-decided(allow) 从 requesting 直接到 active（同意窗可能还没上报 shown，不阻塞会话）。
 */
export function reduceRemoteDesktopSession(
  state: RemoteDesktopSessionState,
  event: RemoteDesktopSessionEvent,
): RemoteDesktopSessionTransition {
  if (event.type === "request") {
    if (state.phase !== "idle") {
      return { state, actions: [], rejected: "busy" };
    }
    const requested: RemoteDesktopSessionState = {
      phase: event.authorized ? "active" : "requesting",
      clientId: event.clientId,
      label: event.label,
      closeReason: null,
    };
    if (event.authorized) {
      // 已记住的设备：跳过同意窗，直接进入会话（spec 产品规则 2）。
      return enterActive(requested, [
        { kind: "answer-consent", outcome: "allow", remember: false },
        { kind: "start-indicator", clientId: event.clientId, label: event.label },
        { kind: "start-capture" },
      ]);
    }
    return {
      state: requested,
      actions: [{ kind: "open-consent", clientId: event.clientId, label: event.label }],
    };
  }

  if (event.type === "consent-shown") {
    if (state.phase !== "requesting") {
      return { state, actions: [] };
    }
    return { state: { ...state, phase: "waitingConsent" }, actions: [] };
  }

  if (event.type === "consent-decided") {
    if (state.phase !== "requesting" && state.phase !== "waitingConsent") {
      return { state, actions: [] };
    }
    const clientId = state.clientId ?? "";
    const label = state.label ?? "";
    if (event.outcome === "allow") {
      return enterActive(
        state,
        withConsentActions(clientId, label, "allow", event.remember),
      );
    }
    return {
      state: { phase: "closed", clientId, label, closeReason: "consent-denied" },
      actions: withConsentActions(clientId, label, event.outcome, false),
    };
  }

  if (event.type === "close") {
    if (state.phase === "closed") {
      // 幂等：首个原因生效，后续 close 不再改理由。
      return { state, actions: [] };
    }
    const actions: RemoteDesktopSessionAction[] = [];
    if (state.phase === "requesting" || state.phase === "waitingConsent") {
      actions.push({ kind: "close-consent" });
    }
    if (state.phase === "active") {
      actions.push({ kind: "stop-capture" }, { kind: "stop-indicator" });
    }
    return {
      state: { ...state, phase: "closed", closeReason: event.reason },
      actions,
    };
  }

  // cleanup-done：收口完成后回到 idle，允许下一个会话（含刚被拒绝过的设备重试）。
  if (state.phase !== "closed") {
    return { state, actions: [] };
  }
  return { state: INITIAL_REMOTE_DESKTOP_SESSION_STATE, actions: [] };
}
