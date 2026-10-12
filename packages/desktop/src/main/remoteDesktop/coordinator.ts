/* eslint-disable max-lines -- 被控会话协调器集中维护状态机解释、同意/指示/采集生命周期与信令转发，拆散会漏收口路径。 */
/**
 * 被控端会话协调器（main 唯一所有者，docs/specs/lan-remote-desktop.md 状态所有者表）。
 *
 * 事件顺序（②③⑤⑥）在 main 侧的落点：
 *
 *   host ──ConsentRequest②──► 本模块 ──同意窗(60s)/已授权直通──► ConsentResult──► host（签发票据）
 *   start-capture ──► 本模块 ──media-open 信令(key/salt) + MediaStart──► host（rd_media 采集，P2）
 *   host ──SignalFromHost③──► 本模块 ──hello→monitors / select-monitor→MediaSwitch+采集窗选源──► host
 *                                   ◄──offer/ice（页面产出）── SignalToHost ──► host ──► 对端
 *   host ──MediaStatus──────► 本模块 ──freeze/unfreeze（reason 归一后）──► 对端（指示条生命周期不受影响）
 *   host ──SessionClosed⑥───► 本模块 ──closeSession(reason) 单出口──► MediaStop + 关采集窗 + 撤销指示 + 清批次
 *   采集窗 ──Input(DataChannel)──► 本模块（校验+换算+按帧批量）── Inject ──► host ──► Helper
 *
 * 收口纪律：所有结束路径（对端 bye / 本地点「结束」/ 同意拒绝 / 应用退出 / WS 关闭 / host 退出）
 * 都只经 `closeSession(reason)`——一个出口，幂等；本地发起的结束先发 `bye` 再清理。
 */
import { randomBytes } from "node:crypto";
import { screen } from "electron";
import type { Locale } from "@mode/shared";
import {
  REMOTE_DESKTOP_PROTOCOL_VERSION,
  type RemoteDesktopSignalMessage,
} from "@mode/shared/remote-desktop";
import {
  RemoteDesktopHostMessageTypes,
  type RemoteDesktopMediaRect,
  type RemoteDesktopRawInputEvent,
} from "./hostMessages.js";
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
  openRemoteDesktopCaptureWindow,
  type RemoteDesktopCaptureSession,
} from "./captureWindow.js";
import { listRemoteDesktopMonitors, physicalRectOf } from "./monitors.js";
import {
  buildSelectMonitorOutbound,
  buildStartCaptureOutbound,
  normalizeMediaFreezeReason,
  type RemoteDesktopMediaOutbound,
} from "./mediaCoordination.js";

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
  /** 会话入口记录的主屏物理矩形：主控尚未 select-monitor 时 MediaStart 的初始采集区域。 */
  pendingDisplayRect: RemoteDesktopMediaRect | null;
  /** 本会话媒体密钥/盐（与 media-open 信令同源下发给 host；解密材料不落日志）。 */
  mediaKey: string | null;
  mediaSalt: string | null;
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

/** 按序执行媒体面出站规划（规划本体是纯函数，顺序契约在 mediaCoordination 单测里锁死）。 */
function dispatchOutbounds(outbounds: readonly RemoteDesktopMediaOutbound[]): void {
  for (const outbound of outbounds) {
    if (outbound.kind === "signal") {
      sendSignalToPeer(outbound.message);
    } else {
      sendRemoteDesktopMessageToHost(outbound.message);
    }
  }
}

/** 主屏矩形读取失败时的开流兜底区域（MediaStart schema 要求必有 rect）。 */
const FALLBACK_MEDIA_RECT: RemoteDesktopMediaRect = { x: 0, y: 0, width: 1920, height: 1080 };

function toMediaRect(physical: {
  xPx: number;
  yPx: number;
  widthPx: number;
  heightPx: number;
}): RemoteDesktopMediaRect {
  return { x: physical.xPx, y: physical.yPx, width: physical.widthPx, height: physical.heightPx };
}

/** 会话入口记录主屏物理矩形（含已授权直通路径）：MediaStart 在主控选屏前用它作初始采集区域。 */
function recordPendingDisplayRect(runtimeState: CoordinatorRuntime): void {
  try {
    runtimeState.pendingDisplayRect = toMediaRect(physicalRectOf(screen.getPrimaryDisplay()));
  } catch (error) {
    // screen 在无显示环境可能不可用：保留 null，start-capture 落到兜底矩形。
    runtimeState.logger.warn("[remote-desktop] read primary display failed:", error);
  }
}

/** select-monitor 的物理矩形解析：displayId 不在本机枚举内 → null（调用方发 error{internal}）。 */
function resolvePhysicalRect(displayId: string): RemoteDesktopMediaRect | null {
  const display = screen.getAllDisplays().find((candidate) => String(candidate.id) === displayId);
  return display ? toMediaRect(physicalRectOf(display)) : null;
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
    // 媒体密钥随会话销毁：不跨会话复用（字符串无法原地清零，只保证不留引用）。
    runtimeState.mediaKey = null;
    runtimeState.mediaSalt = null;
    runtimeState.pendingDisplayRect = null;
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
    case "start-capture": {
      // P2 原生媒体面开流：生成本会话 key/salt 后按规划序列发出（顺序契约在 mediaCoordination：
      // 先 media-open 信令、后 MediaStart，spec 事件顺序 ②）。
      // key/salt 是解密材料：只随消息下发，不落日志（仓库日志纪律）。
      const key = randomBytes(32).toString("hex");
      const salt = randomBytes(8).toString("hex");
      runtimeState.mediaKey = key;
      runtimeState.mediaSalt = salt;
      dispatchOutbounds(
        buildStartCaptureOutbound({ rect: runtimeState.pendingDisplayRect ?? FALLBACK_MEDIA_RECT, key, salt }),
      );
      return;
    }
    case "stop-capture":
      // 采集窗销毁由 closeSession 收口块统一处理（本动作只出现在 close 迁移里）；
      // 这里通知 host 停 helper 采集（rd_media_stop；callRdMedia 由 Task 8 注入，未注入时 host 侧静默跳过）。
      sendRemoteDesktopMessageToHost({
        type: RemoteDesktopHostMessageTypes.RemoteDesktopMediaStop,
      });
      return;
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
  // 会话入口记录主屏物理矩形（含已授权直通路径）：主控尚未 select-monitor 时 MediaStart 用它。
  recordPendingDisplayRect(runtimeState);
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
    // P2 原生媒体面：displayId → 物理 rect 解析后按规划路由（失败 → error{internal}；
    // 成功 → MediaSwitch，helper 换屏）。规划本体在 mediaCoordination（纯函数，单测锁行为）。
    const rect = resolvePhysicalRect(message.displayId);
    if (!rect) {
      runtimeState.logger.warn(`[remote-desktop] unknown monitor displayId: ${message.displayId}`);
    }
    dispatchOutbounds(buildSelectMonitorOutbound(rect));
    if (!rect) {
      return;
    }
    // P1 渲染器采集窗（WebRTC 面）仍在此按 displayId 选源；P4 清理时随 captureWindow 一并删除
    // （spec lan-remote-desktop-native-media.md 删除与保留清单）。
    const capture = await ensureCaptureSession(runtimeState);
    if (!capture?.selectMonitor(message.displayId)) {
      // rect 已对 screen 枚举校验过：失败只剩显示器热插拔竞态，按既有语义报 internal。
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

/** helper kind1 采集状态（host 的 MediaStatus）：冻结透传给主控；指示条生命周期由会话状态管，不受影响。 */
function handleMediaStatus(message: { frozen: boolean; reason?: string }): void {
  const runtimeState = runtime;
  if (!runtimeState || runtimeState.state.phase !== "active") {
    // 会话外的状态通知丢弃（冻结透传只发生在活跃会话）。
    return;
  }
  if (message.frozen) {
    // reason 归一（locked 保真、其余 access-lost）：理由与矩阵断言见 mediaCoordination。
    sendSignalToPeer({ type: "freeze", reason: normalizeMediaFreezeReason(message.reason) });
    return;
  }
  sendSignalToPeer({ type: "unfreeze" });
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
    pendingDisplayRect: null,
    mediaKey: null,
    mediaSalt: null,
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
      handleMediaStatus: (message) => {
        handleMediaStatus(message);
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
