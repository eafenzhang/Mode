/* eslint-disable max-lines -- 被控会话协调器集中维护状态机解释、同意/指示/采集生命周期与信令转发，拆散会漏收口路径。 */
/**
 * 被控端会话协调器（main 唯一所有者，docs/specs/lan-remote-desktop.md 状态所有者表）。
 *
 * 事件顺序（②③⑤⑥）在 main 侧的落点：
 *
 *   host ──ConsentRequest②──► 本模块 ──同意窗(60s)/已授权直通──► ConsentResult──► host（签发票据）
 *   host ──SignalFromHost③──► 本模块 ──hello→monitors / select-monitor→采集──► 隐藏采集窗
 *                                   ◄──offer/ice（页面产出）── SignalToHost ──► host ──► 对端
 *   host ──SessionClosed⑥───► 本模块 ──closeSession(reason) 单出口──► 关采集窗 + 撤销指示 + 清批次
 *   采集窗 ──Input(DataChannel)──► 本模块（校验+换算+按帧批量）── Inject ──► host ──► Helper
 *
 * 收口纪律：所有结束路径（对端 bye / 本地点「结束」/ 同意拒绝 / 应用退出 / WS 关闭 / host 退出）
 * 都只经 `closeSession(reason)`——一个出口，幂等；本地发起的结束先发 `bye` 再清理。
 */
import type { Locale } from "@mode/shared";
import {
  REMOTE_DESKTOP_PROTOCOL_VERSION,
  type RemoteDesktopSignalMessage,
} from "@mode/shared/remote-desktop";
import { RemoteDesktopHostMessageTypes, type RemoteDesktopRawInputEvent } from "./hostMessages.js";
import { configureRemoteDesktopHostBridge, sendRemoteDesktopMessageToHost } from "./hostBridge.js";
import { isRemoteDesktopClientAuthorized } from "./settings.js";
import {
  INITIAL_REMOTE_DESKTOP_SESSION_STATE,
  reduceRemoteDesktopSession,
  type RemoteDesktopSessionAction,
  type RemoteDesktopSessionCloseReason,
  type RemoteDesktopSessionState,
} from "./sessionState.js";
import { openRemoteDesktopConsentWindow, type RemoteDesktopConsentWindow } from "./consentWindow.js";
import {
  openRemoteDesktopIndicatorWindow,
  type RemoteDesktopIndicatorWindow,
} from "./indicatorWindow.js";
import {
  listRemoteDesktopMonitors,
  openRemoteDesktopCaptureWindow,
  type RemoteDesktopCaptureSession,
} from "./captureWindow.js";

export interface RemoteDesktopSessionCoordinatorLogger {
  info(...args: unknown[]): void;
  warn(...args: unknown[]): void;
}

/** 本地发起的结束：先给对端带出 bye，再由 host 关闭 WS。 */
const MAIN_INITIATED_CLOSE_REASONS = new Set<RemoteDesktopSessionCloseReason>([
  "local-end",
  "app-quit",
  "disabled",
  "error",
]);

interface CoordinatorRuntime {
  logger: RemoteDesktopSessionCoordinatorLogger;
  getLocale: () => Locale;
  state: RemoteDesktopSessionState;
  consentWindow: RemoteDesktopConsentWindow | null;
  indicatorWindow: RemoteDesktopIndicatorWindow | null;
  captureSession: RemoteDesktopCaptureSession | null;
  captureOpening: Promise<RemoteDesktopCaptureSession | null> | null;
  closing: boolean;
  listeners: Set<(state: RemoteDesktopSessionState) => void>;
}

let runtime: CoordinatorRuntime | null = null;

export function getRemoteDesktopServerSessionState(): RemoteDesktopSessionState {
  return runtime?.state ?? INITIAL_REMOTE_DESKTOP_SESSION_STATE;
}

export function subscribeRemoteDesktopServerSessionState(
  listener: (state: RemoteDesktopSessionState) => void,
): () => void {
  const target = runtime;
  if (!target) {
    return () => undefined;
  }
  target.listeners.add(listener);
  return () => {
    target.listeners.delete(listener);
  };
}

function notifyState(runtimeState: CoordinatorRuntime): void {
  // Set 迭代器容忍迭代中取消订阅（删除当前元素），无需先复制快照。
  for (const listener of runtimeState.listeners) {
    try {
      listener(runtimeState.state);
    } catch {
      // 订阅方异常不得影响会话收口。
    }
  }
}

function setState(runtimeState: CoordinatorRuntime, next: RemoteDesktopSessionState): void {
  runtimeState.state = next;
  notifyState(runtimeState);
}

function sendConsentResultToHost(
  clientId: string,
  decision: "allow" | "deny",
  remember: boolean,
  reason?: "user" | "timeout",
): void {
  sendRemoteDesktopMessageToHost({
    type: RemoteDesktopHostMessageTypes.RemoteDesktopConsentResult,
    clientId,
    decision,
    remember,
    ...(reason ? { reason } : {}),
  });
}

function sendSignalToPeer(message: RemoteDesktopSignalMessage): void {
  sendRemoteDesktopMessageToHost({
    type: RemoteDesktopHostMessageTypes.RemoteDesktopSignalToHost,
    message,
  });
}

/** 会话结束收口（唯一出口，幂等）：应用状态机动作 → 清理窗口/采集/批次 → 回 idle。 */
export async function closeSession(reason: RemoteDesktopSessionCloseReason): Promise<void> {
  const runtimeState = runtime;
  if (!runtimeState || runtimeState.closing) {
    return;
  }
  runtimeState.closing = true;
  try {
    const transition = reduceRemoteDesktopSession(runtimeState.state, { type: "close", reason });
    setState(runtimeState, transition.state);
    for (const action of transition.actions) {
      await applyAction(runtimeState, action);
    }
    // 本地发起的结束：先把 bye 带出（host 收到 bye 后关闭 WS），对端已 bye/WS 已断时不回发。
    if (MAIN_INITIATED_CLOSE_REASONS.has(reason)) {
      sendSignalToPeer({ type: "bye", reason: "user-end" });
    }
    runtimeState.consentWindow?.close();
    runtimeState.consentWindow = null;
    runtimeState.indicatorWindow?.close();
    runtimeState.indicatorWindow = null;
    const capture = runtimeState.captureSession;
    runtimeState.captureSession = null;
    runtimeState.captureOpening = null;
    if (capture) {
      await capture.close().catch((error: unknown) => {
        runtimeState.logger.warn("[remote-desktop] close capture window failed:", error);
      });
    }
    setState(runtimeState, reduceRemoteDesktopSession(runtimeState.state, { type: "cleanup-done" }).state);
  } finally {
    runtimeState.closing = false;
  }
}

async function ensureCaptureSession(runtimeState: CoordinatorRuntime): Promise<RemoteDesktopCaptureSession | null> {
  if (runtimeState.captureSession) {
    return runtimeState.captureSession;
  }
  if (runtimeState.captureOpening) {
    return await runtimeState.captureOpening;
  }
  const opening = (async (): Promise<RemoteDesktopCaptureSession | null> => {
    try {
      const capture = await openRemoteDesktopCaptureWindow({
        logger: runtimeState.logger,
        handlers: {
          onOutboundSignal: (message) => sendSignalToPeer(message),
          onInput: (events: RemoteDesktopRawInputEvent[]) => {
            if (runtimeState.state.phase !== "active") {
              return;
            }
            sendRemoteDesktopMessageToHost({
              type: RemoteDesktopHostMessageTypes.RemoteDesktopInject,
              events,
            });
          },
          onFatalError: (message) => {
            runtimeState.logger.warn(`[remote-desktop] capture fatal: ${message}`);
            void closeSession("error");
          },
          onDiagnostic: (message) => runtimeState.logger.info(`[remote-desktop] capture diag: ${message}`),
        },
      });
      if (!runtimeState.closing && runtimeState.state.phase === "active") {
        runtimeState.captureSession = capture;
        return capture;
      }
      // 采集窗创建期间会话已收口：直接关掉，不留下无归属窗口。
      await capture.close();
      return null;
    } catch (error) {
      runtimeState.logger.warn("[remote-desktop] open capture window failed:", error);
      void closeSession("error");
      return null;
    } finally {
      runtimeState.captureOpening = null;
    }
  })();
  runtimeState.captureOpening = opening;
  return await opening;
}

async function applyAction(runtimeState: CoordinatorRuntime, action: RemoteDesktopSessionAction): Promise<void> {
  switch (action.kind) {
    case "open-consent": {
      const consentWindow = openRemoteDesktopConsentWindow({
        locale: runtimeState.getLocale(),
        deviceLabel: action.label,
        logger: runtimeState.logger,
      });
      runtimeState.consentWindow = consentWindow;
      // 决策必然收敛：用户点击 / 60s 超时 / 会话被收口（close() 先 settle 再销窗）。
      const decision = await consentWindow.waitForDecision();
      if (runtimeState.consentWindow === consentWindow) {
        runtimeState.consentWindow = null;
      }
      const outcome = decision.kind === "allow" ? "allow" : decision.kind === "timeout" ? "timeout" : "deny";
      const transition = reduceRemoteDesktopSession(runtimeState.state, {
        type: "consent-decided",
        outcome,
        remember: decision.kind === "allow" ? decision.remember : false,
      });
      setState(runtimeState, transition.state);
      for (const next of transition.actions) {
        await applyAction(runtimeState, next);
      }
      if (transition.state.phase === "closed") {
        // 拒绝/超时：没有 WS 会话，只收口同意窗与状态；对端错误码已由 ConsentResult 带出。
        await closeSession("consent-denied");
      }
      return;
    }
    case "close-consent":
      runtimeState.consentWindow?.close();
      runtimeState.consentWindow = null;
      return;
    case "answer-consent": {
      const clientId = runtimeState.state.clientId;
      if (!clientId) {
        return;
      }
      if (action.outcome === "allow") {
        sendConsentResultToHost(clientId, "allow", action.remember);
        return;
      }
      sendConsentResultToHost(clientId, "deny", false, action.outcome === "timeout" ? "timeout" : "user");
      return;
    }
    case "start-indicator": {
      runtimeState.indicatorWindow?.close();
      runtimeState.indicatorWindow = openRemoteDesktopIndicatorWindow({
        locale: runtimeState.getLocale(),
        deviceLabel: action.label,
        logger: runtimeState.logger,
        onEndRequested: () => {
          // 本地结束（spec 产品规则 3：本地用户随时可终止）。
          void closeSession("local-end");
        },
      });
      return;
    }
    case "stop-indicator":
      runtimeState.indicatorWindow?.close();
      runtimeState.indicatorWindow = null;
      return;
    case "start-capture":
      await ensureCaptureSession(runtimeState);
      return;
    case "stop-capture": {
      const capture = runtimeState.captureSession;
      runtimeState.captureSession = null;
      if (capture) {
        await capture.close();
      }
      return;
    }
  }
}

function runActions(runtimeState: CoordinatorRuntime, actions: RemoteDesktopSessionAction[]): void {
  void (async () => {
    for (const action of actions) {
      try {
        await applyAction(runtimeState, action);
      } catch (error) {
        runtimeState.logger.warn("[remote-desktop] session action failed:", error);
      }
    }
  })();
}

async function handleConsentRequest(message: {
  clientId: string;
  label: string;
}): Promise<void> {
  const runtimeState = runtime;
  if (!runtimeState) {
    return;
  }
  // 「已记住」的设备由 main 现读授权列表判定（撤销后立刻恢复弹窗）。
  const authorized = await isRemoteDesktopClientAuthorized(message.clientId);
  const transition = reduceRemoteDesktopSession(runtimeState.state, {
    type: "request",
    clientId: message.clientId,
    label: message.label,
    authorized,
  });
  if (transition.rejected) {
    // host 侧已做单会话互斥；这里是兜底拒绝，避免对端悬挂等待。
    sendConsentResultToHost(message.clientId, "deny", false);
    return;
  }
  setState(runtimeState, transition.state);
  runActions(runtimeState, transition.actions);
}

async function handleSignalFromHost(message: RemoteDesktopSignalMessage): Promise<void> {
  const runtimeState = runtime;
  if (!runtimeState || runtimeState.state.phase !== "active") {
    // 会话外信令一律丢弃（半开状态不允许存在）。
    return;
  }
  if (message.type === "hello") {
    if (message.protocolVersion !== REMOTE_DESKTOP_PROTOCOL_VERSION) {
      sendSignalToPeer({
        type: "error",
        code: "unsupported",
        message: `protocol version ${message.protocolVersion} is not supported`,
      });
      await closeSession("error");
      return;
    }
    try {
      const monitors = await listRemoteDesktopMonitors();
      sendSignalToPeer({ type: "monitors", monitors });
    } catch (error) {
      runtimeState.logger.warn("[remote-desktop] enumerate monitors failed:", error);
      sendSignalToPeer({ type: "error", code: "internal", message: "monitor enumeration failed" });
    }
    return;
  }
  if (message.type === "select-monitor") {
    const capture = await ensureCaptureSession(runtimeState);
    if (!capture || !capture.selectMonitor(message.displayId)) {
      sendSignalToPeer({ type: "error", code: "internal", message: "unknown monitor" });
    }
    return;
  }
  if (message.type === "bye") {
    await closeSession("peer-end");
    return;
  }
  if (message.type === "error") {
    runtimeState.logger.warn(
      `[remote-desktop] peer reported error: ${message.code}${message.message ? ` (${message.message})` : ""}`,
    );
    await closeSession("error");
    return;
  }
  if (message.type === "answer" || message.type === "ice") {
    runtimeState.captureSession?.deliverSignal(message);
    return;
  }
  // 被控端不应收到 monitors/offer（offer 只由本侧发出）：记录但不改状态。
  runtimeState.logger.warn(`[remote-desktop] unexpected signal from peer: ${message.type}`);
}

/** 绑定 hostBridge 的会话回调（main 启动时调用一次）。 */
export function initializeRemoteDesktopSessionCoordinator(options: {
  logger: RemoteDesktopSessionCoordinatorLogger;
  getLocale: () => Locale;
}): void {
  const runtimeState: CoordinatorRuntime = {
    logger: options.logger,
    getLocale: options.getLocale,
    state: INITIAL_REMOTE_DESKTOP_SESSION_STATE,
    consentWindow: null,
    indicatorWindow: null,
    captureSession: null,
    captureOpening: null,
    closing: false,
    listeners: new Set(),
  };
  runtime = runtimeState;
  // 会话生命周期写 info 级日志（spec 安全边界：不记录画面内容）。
  runtimeState.listeners.add((state) => {
    options.logger.info(
      `[remote-desktop] session phase=${state.phase}` +
        (state.clientId ? ` clientId=${state.clientId}` : "") +
        (state.closeReason ? ` reason=${state.closeReason}` : ""),
    );
  });
  configureRemoteDesktopHostBridge({
    logger: options.logger,
    sessionHandlers: {
      handleConsentRequest: (message) => {
        void handleConsentRequest(message);
      },
      handleSignalFromHost: (message) => {
        void handleSignalFromHost(message.message);
      },
      handleSessionClosed: (message) => {
        void closeSession(message.reason === "peer-end" ? "peer-end" : message.reason === "error" ? "error" : "socket-closed");
      },
      handleHostGone: () => {
        void closeSession("socket-closed");
      },
    },
  });
}

/** 应用退出（main before-quit）：会话按 app-quit 收口。 */
export function handleRemoteDesktopAppQuit(): void {
  void closeSession("app-quit");
}

/** 被控总开关关（设置面）：进行中会话立即收口（spec 事件顺序 ⑥）。 */
export function handleRemoteDesktopServerDisabled(): void {
  void closeSession("disabled");
}
