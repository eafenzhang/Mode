import { DatabaseStartupAdmission } from "./databaseStartupAdmission.js";
import { createRoot } from "react-dom/client";
import { useEffect } from "react";
import {
  AppErrorBoundary,
  Root,
  GlobalDatabaseStartupLoading,
  UpdateStatusWindowRoot,
  ModeIntlProvider,
  registerBaseWorkspaceServices,
  registerRemoteWorkspaceSession,
  createRemoteWorkspaceDisconnectedError,
  disposeAllRemoteWorkspaceSessions,
  playTaskNotificationSound,
  setStreamClientId,
} from "@mode/ui";
import "@mode/ui/styles.css";
import {
  createMessagePortServiceConnection,
  type MessagePortServiceConnection,
} from "@mode/client";
import {
  InternalChannels,
  databaseStartupStateSchema,
  type DatabaseStartupControl,
  type DatabaseStartupState,
  parseLaunchMarks,
  LAUNCH_MARKS_QUERY_KEY,
  type LaunchMarks,
  DEFAULT_LOCALE,
} from "@mode/shared";
import type { Locale } from "@mode/shared";
import type { IServiceAccessor } from "@mode/services";
import { createDesktopPlatform, isTauriRendererRuntime } from "./desktopPlatform.js";
import { createShellTerminalService } from "./desktopTerminalBridge.js";
import { isPostBootHostTransportDeath } from "./hostTransportDeath.js";
import {
  HOST_LIVENESS_DEADLINE_MS,
  HOST_LIVENESS_INTERVAL_MS,
  HOST_LIVENESS_STRIKES,
  startHostLivenessWatchdog,
  type HostLivenessWatchdog,
} from "./hostLivenessWatchdog.js";
import { startLocalDisplaySync } from "./desktopLocalDisplays.js";
import { startPerformanceTimelineCleanup } from "./performanceTimelineCleanup.js";
import { buildRemoteWorkspaceSessionServices } from "./remoteWorkspaceSessionServices.js";
import {
  notifyRemoteWorkspaceServicePortReady,
  parseRemoteWorkspaceServicePortMessage,
  type RemoteWorkspaceServicePortRegistration,
} from "./remoteWorkspaceServicePortBridge.js";

type DesktopRendererImportMetaEnv = {
  VITE_MODE_E2E_STORE_BRIDGE?: string;
};

startPerformanceTimelineCleanup();

// T4:renderer bundle 开始执行。同时从 loadURL query 解析 main 注入的 T0-T3。
const rendererStartedAt = Date.now();
const launchMarks: LaunchMarks | null = parseLaunchMarks(
  new URLSearchParams(window.location.search).get(LAUNCH_MARKS_QUERY_KEY),
);
(
  window as Window & {
    __MODE_RENDERER_START__?: number;
    __MODE_LAUNCH_MARKS__?: LaunchMarks | null;
  }
).__MODE_RENDERER_START__ = rendererStartedAt;
(window as Window & { __MODE_LAUNCH_MARKS__?: LaunchMarks | null }).__MODE_LAUNCH_MARKS__ =
  launchMarks;
registerE2EStoreBridgesIfEnabled();

function registerE2EStoreBridgesIfEnabled() {
  const env = ((import.meta as ImportMeta & { env?: DesktopRendererImportMetaEnv }).env ??
    {}) as DesktopRendererImportMetaEnv;
  if (env.VITE_MODE_E2E_STORE_BRIDGE !== "1") {
    return;
  }

  void import("@mode/ui/e2e-store-bridge").then(({ registerE2EStoreBridges }) => {
    registerE2EStoreBridges();
  });
}

// 初始化主题：默认 Zai dark，后续由 useTheme hook 接管
{
  const saved = localStorage.getItem("mode-theme") || "zai-dark";
  const resolved =
    saved === "system"
      ? window.matchMedia("(prefers-color-scheme: dark)").matches
        ? "dark"
        : "light"
      : saved === "dark" || saved === "zai-dark"
        ? "dark"
        : "light";
  const appliedTheme =
    saved === "system"
      ? resolved === "dark"
        ? "zai-dark"
        : "zai-light"
      : saved === "dark"
        ? "zai-dark"
        : saved === "light"
          ? "zai-light"
          : saved;
  if (resolved === "dark") document.documentElement.classList.add("dark");
  document.documentElement.classList.toggle("theme-zai-light", appliedTheme === "zai-light");
  document.documentElement.classList.toggle("theme-zai-dark", appliedTheme === "zai-dark");
}

const isMacDesktop = navigator.userAgent.includes("Mac");
const isWindowsDesktop = navigator.userAgent.includes("Windows");
const isLinuxDesktop = !isMacDesktop && !isWindowsDesktop;
// macOS hidden 原生标题栏仍参与鼠标拖拽命中，深层浮层需要平台标记避开其高度。
document.documentElement.classList.toggle("platform-mac-desktop", isMacDesktop);
// Windows 的原生 titleBarOverlay 与 renderer 共用右上角，深层浮层拿不到
// Root 的 isWindowsDesktop prop 时会把关闭按钮放进原生窗控命中区。根节点平台标记只描述
// Desktop chrome，不会让普通 Windows Web 误用标题栏安全区。
document.documentElement.classList.toggle("platform-windows-desktop", isWindowsDesktop);
// Linux 的窗口标题栏由 renderer 自绘，应用内 Dialog overlay 如果覆盖整个 webContents，
// 会把标题栏点击区一起拦截。给桌面 Linux 根节点打平台标记，让 UI overlay 能只在 Linux 避开标题栏。
document.documentElement.classList.toggle("platform-linux-desktop", isLinuxDesktop);
const isLocalDevelopmentRuntime =
  (globalThis as typeof globalThis & { __MODE_LOCAL_DEVELOPMENT_RUNTIME__?: boolean })
    .__MODE_LOCAL_DEVELOPMENT_RUNTIME__ === true;

function readBooleanFlag(name: string, defaultValue: boolean): boolean {
  const value = new URLSearchParams(window.location.search).get(name);
  if (value == null) return defaultValue;
  return value !== "false" && value !== "0";
}

function readStringFlag(name: string): string | undefined {
  const value = new URLSearchParams(window.location.search).get(name);
  return value == null || value.trim() === "" ? undefined : value;
}

const restoreSession = readBooleanFlag("restoreSession", true);
const supportsSettings = readBooleanFlag("supportsSettings", true);
const initialWorkspaceAbsPath = readStringFlag("initialWorkspacePath");
const initialWorkspacePurpose = readStringFlag("initialWorkspacePurpose");
const unavailableWorkspacePath = readStringFlag("unavailableWorkspacePath");
const windowKind = readStringFlag("windowKind");
const initialLocaleFlag = readStringFlag("locale");
const initialLocale: Locale =
  initialLocaleFlag === "zh-CN" || initialLocaleFlag === "en-US" || initialLocaleFlag === "fa-IR"
    ? initialLocaleFlag
    : DEFAULT_LOCALE;
let baseServicesForRemoteSessions: IServiceAccessor | null = null;
/**
 * base 连接的生命周期句柄：Host 传输死亡（post-boot failed，spec §22）时需要显式
 * dispose 让全部挂起与后续 RPC fail-closed，而不是留在静默 pending 里。
 */
let baseConnection: MessagePortServiceConnection | null = null;
/** post-boot 传输死亡只收口一次；后续 failed 状态不再重复处理。 */
let hostTransportDeathHandled = false;
/** 半开管道心跳（spec §22.5）：只在 base 连接存活期间运行，fail-closed 时停止。 */
let hostLivenessWatchdog: HostLivenessWatchdog | null = null;
const pendingRemoteWorkspaceServicePorts: RemoteWorkspaceServicePortRegistration[] = [];

const desktopPlatform = createDesktopPlatform({
  isLocalDevelopmentRuntime,
  // 端口在 initializeBusinessRoot 才送达；getter 保证 LAN 委托在收到 Host 服务后立即生效。
  getHostServices: () => baseServicesForRemoteSessions,
});
/**
 * 等待 preload 通过 window.postMessage 转发 MessagePort。
 *
 * MessagePort 不能经过 contextBridge（会丢失原生方法），
 * 所以 preload 用 window.postMessage + transfer 把 port 原样传递到 renderer。
 * 本地窗口的 port 来自 utilityProcess，远程窗口也一样，renderer 无需区分。
 */
// 之前用匿名函数注册 addEventListener("message")，reload/HMR 时会重复注册，
// 导致多次 createRoot 在同一 DOM 节点上挂载。用 flag 防止重复初始化。
let appInitialized = false;
const databaseStartupAdmission = new DatabaseStartupAdmission();
const appRoot =
  windowKind === "update-status" ? null : createRoot(document.getElementById("root")!);
const sendStartupControl = (control: DatabaseStartupControl) =>
  window.postMessage({ type: InternalChannels.DatabaseStartupControl, control }, "*");
function renderDatabaseStartup(): void {
  appRoot?.render(
    <AppErrorBoundary isDesktop isMacDesktop={isMacDesktop} isWindowsDesktop={isWindowsDesktop}>
      <ModeIntlProvider
        initialLocale={initialLocaleFlag ? initialLocale : undefined}
        resolveSystemLocale={desktopPlatform.getSystemLocale}
      >
        <StartupReadyNotifier />
        <GlobalDatabaseStartupLoading
          state={databaseStartupAdmission.state}
          onRetry={() => {
            if (databaseStartupAdmission.state)
              sendStartupControl({
                action: "retry",
                attemptId: databaseStartupAdmission.state.attemptId,
              });
          }}
          onCopy={(details) => navigator.clipboard.writeText(details)}
          onExit={() => sendStartupControl({ action: "exit" })}
        />
      </ModeIntlProvider>
    </AppErrorBoundary>,
  );
}
function enterAppIfPrepared(): void {
  if (appInitialized) return;
  const port = databaseStartupAdmission.takeReadyPort();
  if (port) initializeBusinessRoot(port);
}
const firstStartupStateTimer =
  windowKind === "update-status"
    ? undefined
    : setTimeout(() => {
        if (databaseStartupAdmission.state) return;
        const now = Date.now();
        databaseStartupAdmission.state = {
          schemaVersion: 1,
          startupId: "unavailable",
          attemptId: "startup-channel-unavailable",
          sequence: 0,
          startedAt: rendererStartedAt,
          updatedAt: now,
          phase: "failed",
          errorCode: "startup_status_timeout",
          disk: [],
        };
        renderDatabaseStartup();
      }, 30_000);

function registerRemoteWorkspaceServicePort(params: RemoteWorkspaceServicePortRegistration) {
  if (!baseServicesForRemoteSessions) {
    return;
  }

  const remoteConnection = createMessagePortServiceConnection(params.port);
  const remoteServices = remoteConnection.services;
  const services = buildRemoteWorkspaceSessionServices(
    baseServicesForRemoteSessions,
    remoteServices,
    // 权限服务归属依赖对端能力位（键出现即新版对端，docs/specs/lan-remote-computer-use.md）。
    // 注：对象展开 ...(cond ? {…} : {}) 在调用位不是可迭代对象，传 undefined 等价省略第三参。
    params.capabilities ? { capabilities: params.capabilities } : undefined,
  );
  registerRemoteWorkspaceSession({
    sessionId: params.sessionId,
    target: params.target,
    services,
    ...(params.capabilities ? { capabilities: params.capabilities } : {}),
    dispose: (reason) =>
      remoteConnection.dispose(reason ?? createRemoteWorkspaceDisconnectedError()),
  });
  // canonical workspace bind 会换代 remote-scoped port。
  // 只有 store 已注册新 services 后才能确认 ready，bind IPC 返回后的调用方才可重新读取并使用新代 services。
  notifyRemoteWorkspaceServicePortReady(params);
}

function flushPendingRemoteWorkspaceServicePorts(): void {
  if (!baseServicesForRemoteSessions || pendingRemoteWorkspaceServicePorts.length === 0) {
    return;
  }

  const pending = pendingRemoteWorkspaceServicePorts.splice(0);
  for (const entry of pending) {
    registerRemoteWorkspaceServicePort(entry);
  }
}

function StartupReadyNotifier() {
  useEffect(() => {
    // T5:React 首次 commit。供启动分阶段耗时计算 react_commit 段。
    (window as Window & { __MODE_REACT_COMMIT_AT__?: number }).__MODE_REACT_COMMIT_AT__ =
      Date.now();
    // HTML 启动壳的弹出动画结束时，React 首屏可能还没 commit，直接移除壳会露出空白。
    // 这里在 React commit 后通知 index.html，再由启动壳统一判断动画和 React ready 两个条件后退场。
    window.dispatchEvent(new Event("mode-react-startup-ready"));
  }, []);

  return null;
}

function handleServicePortMessage(event: MessageEvent): void {
  if (event.source === window && event.data?.type === InternalChannels.DatabaseStartupState) {
    const result = databaseStartupStateSchema.safeParse(event.data.state);
    if (!result.success) return;
    if (appInitialized) {
      // 启动准入完成后状态流仍在持续推送（bridge.ts 的 pollState 只在 sequence 变化时投递）。
      // Host 进程死亡会在这里送来 phase:"failed"——这是传输死亡唯一抵达渲染层的事实，
      // 必须消费并 fail-closed 收口，而不是像过去那样静默丢弃（spec §22 的缺陷根因）。
      handlePostBootStartupState(result.data);
      return;
    }
    const next = result.data;
    if (!databaseStartupAdmission.acceptState(next)) return;
    if (firstStartupStateTimer) clearTimeout(firstStartupStateTimer);
    renderDatabaseStartup();
    enterAppIfPrepared();
    return;
  }

  if (event.data === InternalChannels.TaskNotificationSound) {
    void playTaskNotificationSound();
    return;
  }

  const remoteWorkspacePort = parseRemoteWorkspaceServicePortMessage(event);
  if (remoteWorkspacePort) {
    if (!baseServicesForRemoteSessions) {
      // renderer reload 时 main 可能先补投 remote port，再投本地 ServicePort。
      // 早到的 remote port 不能直接丢弃，否则 SSH host 仍存活但 UI 会进入断连代理。
      pendingRemoteWorkspaceServicePorts.push(remoteWorkspacePort);
      return;
    }

    registerRemoteWorkspaceServicePort(remoteWorkspacePort);
    return;
  }

  if (
    event.source !== window ||
    event.data?.type !== InternalChannels.ServicePort ||
    appInitialized
  )
    return;
  const port = event.ports[0];
  if (!port) return;
  databaseStartupAdmission.acceptPort({ databaseStartupId: event.data.databaseStartupId }, port);
  enterAppIfPrepared();
}

/**
 * Host 传输死亡（post-boot failed）的 fail-closed 收口（docs/specs/tauri-renderer-host.md §22）。
 *
 * 2026-10-25 验收缺陷：RD 会话异常结束后 Host 进程死亡，壳与桥都如实上报了 failed 状态，
 * 但本模块在 `appInitialized` 后把状态流静默丢弃——ChannelClient 永不 dispose，
 * MessagePort 对端死亡又没有事件，于是全部服务 RPC 永久 pending（横幅 busy 卡死、
 * 模型/LAN 加载永挂）。收口动作：
 * 1. base 连接 dispose：挂起与后续 RPC 立即拒绝（ChannelClient 的 fail-closed 既有机制）；
 * 2. 全部在册 remote scoped session 同步收口（scoped attachment 复用同一条 Host stdio）；
 * 3. 界面切到既有启动失败终态， errorCode 可复制诊断——不允许留在静默挂起里。
 * 恢复语义：进程死亡由壳自动重生 + 整页 reload（§22.4）；「活着但停摆」由心跳 kill（§22.5）。
 */
function handlePostBootStartupState(state: DatabaseStartupState): void {
  const admittedStartupId = databaseStartupAdmission.state?.startupId ?? null;
  if (
    !isPostBootHostTransportDeath(state, {
      appInitialized,
      admittedStartupId,
      alreadyHandled: hostTransportDeathHandled,
    })
  ) {
    return;
  }
  failCloseHostTransport(state);
}

/**
 * 传输死亡收口的唯一实现（§22.2 规则 1-3）：进程死亡（state stream）与心跳停摆（§22.5）
 * 共用同一条路径。state 缺省时复用最近一次准入状态改写失败字段——终态屏需要合法的
 * startupId/attemptId 才能渲染诊断 ID。
 */
function failCloseHostTransport(state?: DatabaseStartupState): void {
  hostTransportDeathHandled = true;
  hostLivenessWatchdog?.stop();
  hostLivenessWatchdog = null;
  const errorCode = state?.errorCode ?? "transport_closed";
  const reason = new Error(`Host 传输已关闭（${errorCode}），服务调用已全部终止`);
  baseConnection?.dispose(reason);
  baseConnection = null;
  disposeAllRemoteWorkspaceSessions(reason);
  // 复用启动失败终态屏：带 errorCode 与诊断复制；与 boot 期失败同一渲染路径。
  const current = databaseStartupAdmission.state;
  databaseStartupAdmission.state = state ?? {
    ...(current ?? {
      schemaVersion: 1,
      startupId: "unknown",
      attemptId: "unknown",
      sequence: 0,
      startedAt: Date.now(),
      updatedAt: Date.now(),
    }),
    phase: "failed",
    errorCode: "transport_closed",
    failedPhase: "starting",
  };
  renderDatabaseStartup();
}

function initializeBusinessRoot(port: MessagePort): void {
  appInitialized = true;
  const connection = createMessagePortServiceConnection(port);
  baseConnection = connection;
  const services = connection.services;
  baseServicesForRemoteSessions = services;
  registerBaseWorkspaceServices(services);
  flushPendingRemoteWorkspaceServicePorts();
  // 半开管道心跳（spec §22.5）：进程死亡走 state stream failed；「活着但事件循环停摆」
  // 只能靠应用层 ping。连续两次拿不到 pong → 先请求壳 kill 当前 Host（走重生换代），
  // 再本地 fail-closed。顺序不能反：dispose 之后渲染层能发出去的只剩桥面控制消息。
  hostLivenessWatchdog = startHostLivenessWatchdog({
    intervalMs: HOST_LIVENESS_INTERVAL_MS,
    deadlineMs: HOST_LIVENESS_DEADLINE_MS,
    strikesToFail: HOST_LIVENESS_STRIKES,
    ping: () => connection.pingHost(HOST_LIVENESS_DEADLINE_MS),
    onStalled: () => {
      // 停摆的可观测性在壳侧（kill → 重生会打 `Host 重生：startupId=…`， Host 日志可见）。
      try {
        sendStartupControl({ action: "kill" });
      } catch {
        // 控制消息发不出去也得本地收口；壳侧即使没收到，重载/退出仍由用户兜底。
      }
      failCloseHostTransport("transport_closed");
    },
  });
  // LR-3 被控侧：把显示器快照推给 Host（Tauri 下才生效；Electron 无此服务）。
  startLocalDisplaySync(services);
  // 终端 PTY 下沉（spec `terminal-pty-in-shell.md`）：Tauri 下用壳-backed 实现覆盖
  // terminalService——profile 仍由 Host 提供，进程在壳侧，Host 崩溃/重生不再带走终端。
  // registerBaseWorkspaceServices/baseServicesForRemoteSessions 保持原服务（远端
  // scoped session 用远端 Host 的终端，不适用本覆盖）。
  let uiServices: IServiceAccessor = services;
  if (isTauriRendererRuntime() && services.terminalService.profile) {
    const hostProfile = services.terminalService.profile.bind(services.terminalService);
    const shellTerminal = createShellTerminalService({ profile: hostProfile });
    uiServices = new Proxy(services, {
      get(target, prop, receiver) {
        if (prop === "terminalService") {
          return shellTerminal;
        }
        return Reflect.get(target, prop, receiver);
      },
    });
  }
  const settingService = supportsSettings ? services.settingService : undefined;
  // 初始化稳定的设备 ID，确保所有 hook 在首次渲染前就使用正确的值
  setStreamClientId(desktopPlatform.getDeviceId());

  appRoot?.render(
    <AppErrorBoundary isDesktop isMacDesktop={isMacDesktop} isWindowsDesktop={isWindowsDesktop}>
      <ModeIntlProvider
        settingService={settingService}
        broadcastService={services.broadcastService}
        resolveSystemLocale={desktopPlatform.getSystemLocale}
      >
        <StartupReadyNotifier />
        <Root
          services={uiServices}
          platform={desktopPlatform}
          isDesktop
          assistantCodeCommentCardsEnabled
          isMacDesktop={isMacDesktop}
          isWindowsDesktop={isWindowsDesktop}
          restoreSession={restoreSession}
          supportsSettings={supportsSettings}
          initialWorkspaceAbsPath={initialWorkspaceAbsPath}
          initialWorkspacePurpose={
            initialWorkspacePurpose === "conversation" ? "conversation" : "project"
          }
          unavailableWorkspacePath={unavailableWorkspacePath}
        />
      </ModeIntlProvider>
    </AppErrorBoundary>,
  );
}

window.addEventListener("message", handleServicePortMessage);
if (windowKind !== "update-status") {
  renderDatabaseStartup();
  sendStartupControl({ action: "snapshot" });
}

if (windowKind === "update-status") {
  createRoot(document.getElementById("root")!).render(
    <AppErrorBoundary isDesktop isMacDesktop={isMacDesktop} isWindowsDesktop={isWindowsDesktop}>
      <StartupReadyNotifier />
      <UpdateStatusWindowRoot
        platform={desktopPlatform}
        initialLocale={initialLocale}
        onRequestClose={() => window.close()}
      />
    </AppErrorBoundary>,
  );
}
