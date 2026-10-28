/* eslint-disable max-lines -- Web 入口集中编排启动、路由与 workspace shell wiring，与 Root.tsx 同样先保持入口收口，避免跨层状态拆散。 */
import { useState } from "react";
import { createRoot } from "react-dom/client";
import {
  AppErrorBoundary,
  Root,
  ModeIntlProvider,
  generateMobileDeviceFingerprint,
  playTaskNotificationSound,
  registerRemoteWorkspaceSession,
  setStreamClientId,
  unregisterRemoteWorkspaceSession,
  type Theme,
} from "@mode/ui";
import "@mode/ui/styles.css";
import { connectViaWebSocket } from "@mode/client";
import { WebCallbackPage } from "./auth/WebCallbackPage.js";
import { createWebAuthService } from "./auth/webAuthService.js";
import { parseOAuthState, resolveSafeAppReturnTo } from "./auth/oauthStateCodec.js";
import { resolveWebCommunityUrl } from "./communityUrl.js";
import type {
  CreateTempTextAttachmentResult,
  FnosAppUpdateApplyResult,
  FnosAppUpdateState,
  IPlatformService,
  LanAccessPairRequest,
  LanAccessPairResult,
  LanDiscoveredPeer,
  LanPairedPeer,
  LanPairedPeerWorkspaces,
  RemoteSessionClosedEvent,
  RemoteTarget,
  ServerRemoteInfo,
} from "@mode/shared";
import { lanAccessStateSchema, type LanAccessState } from "@mode/shared";
import type {
  RemoteDesktopCodec,
  RemoteDesktopErrorCode,
  RemoteDesktopQuality,
  RemoteDesktopSignalMessage,
} from "@mode/shared/remote-desktop";
import { WEB_DEFAULT_THEME, resolveWebInitialTheme } from "./webThemeSeed.js";

function resolveWebThemePreference(defaultTheme: Theme = WEB_DEFAULT_THEME): Theme {
  const saved = localStorage.getItem("mode-theme");
  return resolveWebInitialTheme({ storedTheme: saved, defaultTheme });
}

// 初始化主题：默认 Zai dark，后续由 useTheme hook 接管
// system 模式下需要查询系统偏好；非 system 模式直接用存储值
{
  const saved = resolveWebThemePreference();
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
  document.documentElement.classList.toggle("dark", resolved === "dark");
  document.documentElement.classList.toggle("theme-zai-light", appliedTheme === "zai-light");
  document.documentElement.classList.toggle("theme-zai-dark", appliedTheme === "zai-dark");
}

const root = createRoot(document.getElementById("root")!);
const webAuthService = createWebAuthService();

// 初始化 Web 端流式 clientId，确保所有 hook 在首次渲染前就使用稳定 ID
{
  setStreamClientId(generateMobileDeviceFingerprint());
}

interface WebBootstrapResult {
  wsUrl: string;
  initialWorkspaceAbsPath?: string;
  initialWorkspaceIdentity?: string;
  initialTaskId?: string;
  restoreSession?: boolean;
  allowOpenWorkspace?: boolean;
  /** 服务端是否装配 LAN 主控（`/api/lan/peers`、`/api/connect-remote-lan`）。 */
  supportsLanPeerController?: boolean;
}

function isWebOAuthCallback(params: URLSearchParams): boolean {
  return (
    ["/cn/share/callback", "/share/callback"].includes(window.location.pathname) &&
    params.has("state") &&
    (params.has("code") || params.has("error"))
  );
}

function renderWebAuthCallbackPage(): void {
  document.title = "Mode - Sign In";
  const callbackState = parseOAuthState(
    new URLSearchParams(window.location.search).get("state") ?? "",
  );
  const safeRetryTarget = resolveSafeAppReturnTo(callbackState?.app_return_to);
  root.render(
    <WebCallbackPage
      authService={webAuthService}
      onSuccess={({ appReturnTo }) => {
        window.location.replace(appReturnTo ?? "/");
      }}
      onRetry={() => {
        window.location.replace(safeRetryTarget ?? "/");
      }}
    />,
  );
}

/**
 * 局域网访问（服务端视角）在 Web/NAS 形态由服务端 lanAccessHost 提供
 * （docs/specs/lan-host-server-mode.md）：这里只做 base 感知的 fetch + shared schema 校验，
 * 状态与动作的唯一所有者仍是服务端。
 */
async function requestLanAccessState(path: string, init?: RequestInit): Promise<LanAccessState> {
  const response = await fetch(`${resolveBasePath()}/api/lan/${path}`, {
    ...init,
    ...(init?.body ? { headers: { "content-type": "application/json" } } : {}),
  });
  if (!response.ok) {
    let message = `HTTP ${response.status}`;
    try {
      const payload = (await response.json()) as { error?: unknown };
      if (typeof payload.error === "string" && payload.error) {
        message = payload.error;
      }
    } catch {
      // 非 JSON 错误体：保留状态码。
    }
    throw new Error(message);
  }
  const parsed = lanAccessStateSchema.safeParse(await response.json());
  if (!parsed.success) {
    throw new Error("局域网访问状态响应不符合协议");
  }
  return parsed.data;
}

async function requestFnosAppUpdate(
  path: string,
  method: "GET" | "POST" = "GET",
): Promise<FnosAppUpdateState> {
  const response = await fetch(`${resolveBasePath()}/api/app-update/${path}`, {
    method,
    cache: "no-store",
  });
  // 桌面 Host 的 LAN 静态站也复用 Web 前端，但它不装配 fpk 更新端点；
  // 404 视为「该部署不支持」，让设置卡片保持隐藏，而不是报一个无意义错误。
  if (response.status === 404) {
    return { enabled: false, currentVersion: "", phase: "idle", canApply: false };
  }
  if (!response.ok) {
    throw new Error(`HTTP ${response.status}`);
  }
  return (await response.json()) as FnosAppUpdateState;
}

async function downloadLogsExport(): Promise<{ success: boolean; path?: string; error?: string }> {
  const response = await fetch(`${resolveBasePath()}/api/logs/export`, { cache: "no-store" });
  if (response.status === 404) {
    return { success: false, error: "no_logs" };
  }
  if (!response.ok) {
    return { success: false, error: `http_${String(response.status)}` };
  }
  const disposition = response.headers.get("content-disposition") ?? "";
  const fileName = /filename="([^"]+)"/u.exec(disposition)?.[1] ?? "Mode-logs.log";
  const blob = await response.blob();
  const objectUrl = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = objectUrl;
  anchor.download = fileName;
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  globalThis.setTimeout(() => URL.revokeObjectURL(objectUrl), 10_000);
  return { success: true, path: fileName };
}

async function requestApplyFnosAppUpdate(): Promise<FnosAppUpdateApplyResult> {
  const response = await fetch(`${resolveBasePath()}/api/app-update/apply`, {
    method: "POST",
    cache: "no-store",
  });
  if (response.status === 404) {
    return { ok: false, restarting: false, error: "disabled" };
  }
  if (!response.ok) {
    throw new Error(`HTTP ${response.status}`);
  }
  return (await response.json()) as FnosAppUpdateApplyResult;
}

const lanRemoteSockets = new Map<string, WebSocket>();
const remoteSessionClosedHandlers = new Set<(event: RemoteSessionClosedEvent) => void>();

function emitRemoteSessionClosed(event: RemoteSessionClosedEvent): void {
  for (const handler of remoteSessionClosedHandlers) {
    handler(event);
  }
}

async function fetchLanPeerJson<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`${resolveBasePath()}/api/lan/peers${path}`, {
    cache: "no-store",
    ...init,
  });
  if (!response.ok) {
    let message = `HTTP ${String(response.status)}`;
    try {
      const payload = (await response.json()) as { error?: unknown };
      if (typeof payload.error === "string" && payload.error) {
        message = payload.error;
      }
    } catch {
      // 非 JSON 错误体：保留状态码。
    }
    throw new Error(message);
  }
  return (await response.json()) as T;
}

type RemoteDesktopSessionEvent = Parameters<
  Parameters<IPlatformService["onRemoteDesktopSessionEvent"]>[0]
>[0];

interface ActiveRemoteDesktopSession {
  id: string;
  signal: WebSocket | null;
  closedNotified: boolean;
  disposed: boolean;
}

const REMOTE_DESKTOP_ERROR_CODES = new Set<string>([
  "consent_denied",
  "consent_timeout",
  "disabled",
  "busy",
  "unsupported",
  "unauthorized",
  "stale_ticket",
  "locally_controlled",
  "decoder_unsupported",
  "internal",
]);

function toRemoteDesktopErrorCode(value: unknown): RemoteDesktopErrorCode {
  return typeof value === "string" && REMOTE_DESKTOP_ERROR_CODES.has(value)
    ? (value as RemoteDesktopErrorCode)
    : "internal";
}

/**
 * 浏览器解码能力预检（WebCodecs + WebCrypto）：媒体帧由 nativeMediaSession 用
 * VideoDecoder 解码、crypto.subtle 解密。两者都是 secure-context API：
 * `http://<NAS 地址>:3030` 直连时浏览器不提供，表现为「解码不了」——必须在发请求前
 * 拦住，否则对端先弹同意框、建好中继后才在本地失败。缺失时统一报
 * `decoder_unsupported`，文案同时提示改用 HTTPS（飞牛网关）或 localhost。
 */
function browserCanDecodeRemoteMedia(): boolean {
  return (
    typeof globalThis.VideoDecoder === "function" &&
    typeof globalThis.crypto?.subtle === "object" &&
    globalThis.crypto.subtle !== null
  );
}

const remoteDesktopSessionHandlers = new Set<(event: RemoteDesktopSessionEvent) => void>();
let activeRemoteDesktopSession: ActiveRemoteDesktopSession | null = null;

function emitRemoteDesktopSessionEvent(event: RemoteDesktopSessionEvent): void {
  for (const handler of remoteDesktopSessionHandlers) {
    handler(event);
  }
}

function closeActiveRemoteDesktopSession(options: {
  endOnServer: boolean;
  emitClosed: boolean;
}): void {
  const session = activeRemoteDesktopSession;
  if (!session) {
    return;
  }
  activeRemoteDesktopSession = null;
  session.disposed = true;
  session.signal?.close();
  if (options.endOnServer) {
    void fetch(`${resolveBasePath()}/api/remote-desktop/peer-session/${session.id}/end`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ reason: "user-end" }),
    }).catch(() => undefined);
  }
  if (options.emitClosed && !session.closedNotified) {
    emitRemoteDesktopSessionEvent({ kind: "state", state: "closed", retryable: false });
  }
}

function createWebPlatform(supportsLanPeerController = false): IPlatformService {
  return {
    canSelectFilePath: false,
    supportsLanPeerController,
    // 浏览器没有“在系统文件管理器中显示”的入口；隐藏 Help 菜单里的资源管理器项。
    supportsResourceManager: false,
    // Web 端无法打开系统目录选择框
    selectDirectory: () => Promise.resolve(null),
    // Web 端无法打开系统文件选择框
    selectFile: () => Promise.resolve(null),
    selectFiles: () => Promise.resolve([]),
    getPathForFile: () => null,
    createTempTextAttachment: async (payload) => {
      const response = await fetch(`${resolveBasePath()}/api/attachments/temp-text`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(payload),
      });
      if (!response.ok) {
        throw new Error(`HTTP ${String(response.status)}`);
      }
      return (await response.json()) as CreateTempTextAttachmentResult;
    },
    onRemoteConnectionLog: () => () => {},
    onRemoteSessionClosed: (handler) => {
      remoteSessionClosedHandlers.add(handler);
      return () => {
        remoteSessionClosedHandlers.delete(handler);
      };
    },
    onBotRemoteWorkspaceReconnected: () => () => {},
    // Web 端无多窗口管理
    activateOrSetWorkspace: () => Promise.resolve({ activated: false }),
    /**
     * LAN 主控（docs/specs/web-remote-workspace.md）：
     * POST 让服务端 attach 对端 `/ws/host` → 浏览器再连 `/ws/remote/:id` 拿完整 services
     * → 注册进 remoteWorkspaceSessionStore，复用现有向导/目录浏览器/远程 tab。
     */
    async connectRemote(options: RemoteTarget) {
      if (options.kind !== "lan") {
        return {
          success: false,
          error: `Remote ${options.kind} is not supported in Web mode yet`,
        };
      }
      const response = await fetch(`${resolveBasePath()}/api/connect-remote-lan`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(options),
      });
      if (!response.ok) {
        let message = `HTTP ${String(response.status)}`;
        try {
          const payload = (await response.json()) as { error?: unknown };
          if (typeof payload.error === "string" && payload.error) {
            message = payload.error;
          }
        } catch {
          // 非 JSON 错误体：保留状态码。
        }
        return { success: false, error: message };
      }
      const { remoteSessionId } = (await response.json()) as { remoteSessionId?: unknown };
      if (typeof remoteSessionId !== "string" || !remoteSessionId) {
        return { success: false, error: "Remote session was not created" };
      }
      let disposed = false;
      try {
        const services = await connectViaWebSocket(
          `${resolveDefaultWsOrigin()}/ws/remote/${remoteSessionId}`,
          {
            onOpenSocket: (ws) => {
              lanRemoteSockets.set(remoteSessionId, ws);
            },
            onClose: (event) => {
              // 对端/网络断开：摘掉 renderer 侧 session；server 侧由 WS close 自行 dispose。
              // 用户主动 dispose 会先把 disposed 置 true，此时不再派发「异常关闭」事件。
              const wasDisposed = disposed;
              lanRemoteSockets.delete(remoteSessionId);
              unregisterRemoteWorkspaceSession(remoteSessionId);
              if (!wasDisposed) {
                emitRemoteSessionClosed({
                  sessionId: remoteSessionId,
                  reason: "host-exit",
                  exitCode: null,
                  signal: null,
                  ...(event.reason ? { error: event.reason } : {}),
                });
              }
            },
          },
        );
        const dispose = () => {
          if (disposed) {
            return;
          }
          disposed = true;
          const current = lanRemoteSockets.get(remoteSessionId);
          lanRemoteSockets.delete(remoteSessionId);
          current?.close();
          void fetch(`${resolveBasePath()}/api/remote-workspace/${remoteSessionId}/release`, {
            method: "POST",
          }).catch(() => undefined);
        };
        registerRemoteWorkspaceSession({
          sessionId: remoteSessionId,
          target: options,
          services,
          dispose,
        });
        return { success: true, sessionId: remoteSessionId };
      } catch (error) {
        lanRemoteSockets.get(remoteSessionId)?.close();
        lanRemoteSockets.delete(remoteSessionId);
        await fetch(`${resolveBasePath()}/api/remote-workspace/${remoteSessionId}/release`, {
          method: "POST",
        }).catch(() => undefined);
        return {
          success: false,
          error: error instanceof Error ? error.message : String(error),
        };
      }
    },
    cancelPendingRemoteConnection: (_requestId?: string) => Promise.resolve(),
    disposeRemoteSession: (sessionId: string) => {
      // store 的 unregister 会调用 session.dispose（关闭 WS + server release）。
      unregisterRemoteWorkspaceSession(sessionId);
      lanRemoteSockets.delete(sessionId);
      return Promise.resolve();
    },
    isDockerAvailable: () => Promise.resolve(false),
    listWSLDistros: () => Promise.resolve([]),
    listDockerContainers: () => Promise.resolve([]),
    listSSHConfigAliases: () => Promise.resolve([]),
    // 局域网访问（服务端视角）：Web/NAS 形态由服务端 lanAccessHost 提供
    // （docs/specs/lan-host-server-mode.md）；浏览器只经 /api/lan/* 读写状态。
    getLanAccessState: () => requestLanAccessState("state"),
    setLanAccessEnabled: (enabled: boolean) =>
      requestLanAccessState("enable", { method: "POST", body: JSON.stringify({ enabled }) }),
    createLanAccessPairCode: () => requestLanAccessState("pair-code", { method: "POST" }),
    removeLanAccessClient: (clientId: string) =>
      requestLanAccessState(`clients/${encodeURIComponent(clientId)}`, { method: "DELETE" }),
    resetLanAccessTokens: () => requestLanAccessState("reset", { method: "POST" }),
    discoverLanPeers: () => fetchLanPeerJson<LanDiscoveredPeer[]>("/discover", { method: "POST" }),
    async pairLanPeer(request: LanAccessPairRequest): Promise<LanAccessPairResult> {
      const peer = await fetchLanPeerJson<LanPairedPeer>("/pair", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(request),
      });
      // 令牌留在 server credential service；UI 不使用返回的 token，置空避免误传播。
      return {
        token: "",
        serverId: peer.serverId,
        ...(peer.name ? { name: peer.name } : {}),
      };
    },
    getLanPairedPeers: () => fetchLanPeerJson<LanPairedPeer[]>("/"),
    getLanPeerWorkspaces: (serverId: string) =>
      fetchLanPeerJson<LanPairedPeerWorkspaces>(
        `/${encodeURIComponent(serverId)}/workspaces`,
      ),
    removeLanPairedPeer: (serverId: string) =>
      fetchLanPeerJson<{ ok: boolean }>(`/${encodeURIComponent(serverId)}`, {
        method: "DELETE",
      }).then(() => undefined),
    renameLanPairedPeer: (serverId: string, name: string) =>
      fetchLanPeerJson<{ ok: boolean }>(`/${encodeURIComponent(serverId)}/rename`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ name }),
      }).then(() => undefined),
    // 远程桌面主控（docs/specs/web-remote-desktop.md）：服务端中继对端信令/媒体，
    // 浏览器用现有 nativeMediaSession 解密/WebCodecs 解码；令牌不出服务端。
    async rdGetPeerCapability(serverId: string) {
      try {
        const response = await fetch(
          `${resolveBasePath()}/api/remote-desktop/peer-capability?serverId=${encodeURIComponent(serverId)}`,
          { cache: "no-store" },
        );
        if (!response.ok) {
          return { online: false, remoteDesktop: false, remoteDesktopNative: false };
        }
        return (await response.json()) as Awaited<
          ReturnType<IPlatformService["rdGetPeerCapability"]>
        >;
      } catch {
        return { online: false, remoteDesktop: false, remoteDesktopNative: false };
      }
    },
    async rdStartSession(
      serverId: string,
      quality?: RemoteDesktopQuality,
      codec?: RemoteDesktopCodec,
    ) {
      // 本机能力先于对端：不支持 WebCodecs 时不发起请求（对端不会收到同意弹窗）。
      if (!browserCanDecodeRemoteMedia()) {
        emitRemoteDesktopSessionEvent({
          kind: "state",
          state: "closed",
          error: "decoder_unsupported",
          retryable: false,
        });
        return { error: "decoder_unsupported" as const };
      }
      closeActiveRemoteDesktopSession({ endOnServer: true, emitClosed: false });
      emitRemoteDesktopSessionEvent({ kind: "state", state: "connecting" });
      let response: Response;
      try {
        response = await fetch(`${resolveBasePath()}/api/remote-desktop/peer-session`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ serverId, quality, codec }),
        });
      } catch {
        emitRemoteDesktopSessionEvent({
          kind: "state",
          state: "closed",
          error: "internal",
          retryable: false,
        });
        return { error: "internal" as const };
      }
      let payload: unknown = null;
      try {
        payload = await response.json();
      } catch {
        payload = null;
      }
      const record = (payload ?? {}) as {
        sessionId?: unknown;
        signalPath?: unknown;
        mediaPath?: unknown;
        errorCode?: unknown;
      };
      if (
        !response.ok ||
        typeof record.sessionId !== "string" ||
        !record.sessionId ||
        typeof record.signalPath !== "string" ||
        typeof record.mediaPath !== "string"
      ) {
        const code = toRemoteDesktopErrorCode(record.errorCode);
        emitRemoteDesktopSessionEvent({
          kind: "state",
          state: "closed",
          error: code,
          retryable: false,
        });
        return { error: code };
      }

      const session: ActiveRemoteDesktopSession = {
        id: record.sessionId,
        signal: null,
        closedNotified: false,
        disposed: false,
      };
      activeRemoteDesktopSession = session;
      const signal = new WebSocket(`${resolveDefaultWsOrigin()}${record.signalPath}`);
      session.signal = signal;
      signal.onmessage = (event) => {
        if (activeRemoteDesktopSession !== session || session.disposed) {
          return;
        }
        let envelope: unknown;
        try {
          envelope = JSON.parse(String(event.data));
        } catch {
          return;
        }
        const kind = (envelope as { kind?: unknown }).kind;
        if (kind === "state") {
          const state = (envelope as { state?: unknown }).state;
          if (state === "closed") {
            session.closedNotified = true;
          }
          const error = (envelope as { error?: unknown }).error;
          const retryable = (envelope as { retryable?: unknown }).retryable;
          emitRemoteDesktopSessionEvent({
            kind: "state",
            state: state === "connected" ? "connected" : "closed",
            ...(typeof error === "string" ? { error: toRemoteDesktopErrorCode(error) } : {}),
            ...(typeof retryable === "boolean" ? { retryable } : {}),
          });
          return;
        }
        if (kind !== "signal") {
          return;
        }
        const message = (envelope as { message?: unknown }).message;
        if (
          message &&
          typeof message === "object" &&
          typeof (message as { type?: unknown }).type === "string"
        ) {
          emitRemoteDesktopSessionEvent({
            kind: "signal",
            message: message as RemoteDesktopSignalMessage,
          });
        }
      };
      signal.onclose = () => {
        if (activeRemoteDesktopSession !== session || session.disposed) {
          return;
        }
        activeRemoteDesktopSession = null;
        if (!session.closedNotified) {
          emitRemoteDesktopSessionEvent({
            kind: "state",
            state: "closed",
            retryable: true,
          });
        }
      };
      signal.onerror = () => {
        // close 事件随后到。
      };
      return {
        ok: true as const,
        mediaUrl: `${resolveDefaultWsOrigin()}${record.mediaPath}`,
      };
    },
    rdGetControlledState: () => Promise.resolve({ controlled: false }),
    rdSendSignal: (message: RemoteDesktopSignalMessage) => {
      const session = activeRemoteDesktopSession;
      if (!session?.signal || session.signal.readyState !== WebSocket.OPEN) {
        return;
      }
      session.signal.send(JSON.stringify({ kind: "signal", message }));
    },
    rdEndSession: (reason?: "user-end" | "error") => {
      const session = activeRemoteDesktopSession;
      if (session?.signal && session.signal.readyState === WebSocket.OPEN) {
        session.signal.send(
          JSON.stringify({ kind: "signal", message: { type: "bye", reason } }),
        );
      }
      closeActiveRemoteDesktopSession({ endOnServer: true, emitClosed: false });
      return Promise.resolve();
    },
    rdReadClipboardText: async () => {
      try {
        return await navigator.clipboard.readText();
      } catch {
        return "";
      }
    },
    rdGetServerState: () =>
      Promise.resolve({ enabled: false, authorizedClients: [], encoderCapability: "unknown" }),
    rdSetServerEnabled: () => Promise.reject(new Error("Web mode does not serve remote desktop")),
    rdRevokeClient: () => Promise.reject(new Error("Web mode does not serve remote desktop")),
    rdAuthorizeClient: () => Promise.reject(new Error("Web mode does not serve remote desktop")),
    onRemoteDesktopSessionEvent: (handler) => {
      remoteDesktopSessionHandlers.add(handler);
      return () => {
        remoteDesktopSessionHandlers.delete(handler);
      };
    },
    loadMcpFromUserDirectory: () => Promise.resolve({ servers: [] }),
    saveMcpToUserDirectory: () =>
      Promise.resolve({
        success: false,
        error: "MCP native directory management requires a desktop attachment",
      }),
    migrateLegacyCommonMcp: () =>
      Promise.resolve({
        servers: {},
        totalCount: 0,
        importedCount: 0,
        skippedCount: 0,
      }),
    openExternal: (url) => {
      window.open(url, "_blank", "noopener,noreferrer");
    },
    openCommunity: async () => {
      // fa-IR 等非中文语言跟随英文社区入口。
      const locale = document.documentElement.lang === "zh-CN" ? "zh-CN" : "en-US";
      const communityUrl = await resolveWebCommunityUrl(locale);
      if (!communityUrl) {
        return;
      }
      window.open(communityUrl, "_blank", "noopener,noreferrer");
    },
    canOpenCommunity: async (locale) => {
      const communityUrl = await resolveWebCommunityUrl(locale);
      return typeof communityUrl === "string" && communityUrl.length > 0;
    },
    openInFileManager: () =>
      Promise.resolve({ success: false, error: "Not supported in web mode" }),
    openExternalFile: () => Promise.resolve({ success: false, error: "Not supported in web mode" }),
    registerOAuthState: (_payload) => {},
    onOAuthCallback: () => () => {},
    onPaymentCallback: () => () => {},
    onShareImport: () => () => {},
    notifyRendererReady: () => {},
    showTaskNotification: (payload) => {
      if (document.hasFocus()) {
        return;
      }

      if (
        typeof window.Notification === "undefined" ||
        window.Notification.permission !== "granted"
      ) {
        return;
      }

      try {
        new window.Notification(payload.title, {
          body: payload.body,
          silent: true,
        });
        void playTaskNotificationSound();
      } catch {
        // 浏览器通知不可用时静默忽略，避免打断主流程
      }
    },
    // Web 端不需要跨窗口 tab 管理
    syncWindowTabs: () => {},
    // Web 端没有宿主层 Dock / 任务栏徽标，保持空实现以兼容统一平台接口
    syncWindowUnreadCount: () => {},
    // Web 无宿主窗口行为（关闭即关页），同步设置为空操作（接口注释的 fallback 语义）
    syncAppSettings: () => {},
    syncActiveTaskSession: () => {},
    onFocusTab: () => () => {},
    onNewTab: () => () => {},
    onCloseActiveContextRequest: () => () => {},
    onOpenBrowserUrl: () => () => {},
    onNewTask: () => () => {},
    onOpenWorkspace: () => () => {},
    onWindowFullscreenChanged: () => () => {},
    onTaskNotificationClick: () => () => {},
    exportLogs: () => downloadLogsExport(),
    importChromeBrowserData: (_options) =>
      Promise.resolve({
        success: false,
        cookies: { imported: 0, skipped: 0, failed: 0 },
        localStorage: {
          originsImported: 0,
          entriesImported: 0,
          originsSkipped: 0,
          originsFailed: 0,
        },
        error: "chrome_import_not_supported" as const,
      }),
    clearEmbeddedBrowserData: () =>
      Promise.resolve({ success: false, error: "Not supported in web mode" }),
    // IPlatformService 新增更新提示能力后，Web fallback 没有同步补齐空实现，
    // 根级 typecheck 会直接失败，连与桌面端无关的改动都没法完成校验。
    // Web 端当前没有桌面更新器，先显式 no-op，保持接口完整且不改变现有行为。
    onUpdateReady: () => () => {},
    onUpdateCheckResult: () => () => {},
    onUpdateStateChanged: () => () => {},
    getUpdateState: () => Promise.resolve({ kind: "idle", enabled: true }),
    downloadUpdate: () => Promise.resolve(),
    cancelUpdateDownload: () => Promise.resolve(),
    getAppVersion: async () => {
      const response = await fetch(`${resolveBasePath()}/api/server-info`, { cache: "no-store" });
      if (!response.ok) {
        throw new Error(`HTTP ${String(response.status)}`);
      }
      const info = (await response.json()) as { version?: string };
      return typeof info.version === "string" ? info.version : "unknown";
    },
    getFnosAppUpdateState: () => requestFnosAppUpdate("state"),
    checkFnosAppUpdate: () => requestFnosAppUpdate("check", "POST"),
    downloadFnosAppUpdate: () => requestFnosAppUpdate("download", "POST"),
    applyFnosAppUpdate: () => requestApplyFnosAppUpdate(),
    getDesktopSessionActivity: () => Promise.resolve({ runningAgentSessionCount: 0 }),
    getDesktopZoomLevel: () => Promise.resolve({ zoomLevel: 0 }),
    onDesktopZoomLevelChanged: () => () => {},
    onPostUpdateReleaseNotes: () => () => {},
    acknowledgePostUpdateReleaseNotes: () => Promise.resolve(),
    skipUpdateVersion: () => Promise.resolve(),
    quitAndInstallUpdate: () => Promise.resolve(),
    getInstalledEditors: () => Promise.resolve([]),
    openInEditor: () => Promise.resolve({ success: false, error: "Not supported in web mode" }),
    executeDesktopCommand: () => Promise.resolve(),
    setApplicationLocale: (_locale) => Promise.resolve(),
    setTitleBarTheme: () => Promise.resolve(),
    getDeviceId: () => {
      const nav = globalThis.navigator as Navigator & { platform?: string };
      const platform = nav?.platform ?? "";
      const screenWidth = globalThis.screen?.width;
      const screenHeight = globalThis.screen?.height;
      const colorDepth = globalThis.screen?.colorDepth;
      const parts = [
        platform,
        screenWidth !== undefined ? String(screenWidth) : "",
        screenHeight !== undefined ? String(screenHeight) : "",
        colorDepth !== undefined ? String(colorDepth) : "",
      ];
      return parts.filter(Boolean).join("|");
    },
  };
}

// vite base 可能不是根（fnOS 统一网关把应用挂在 /app/mode/ 子路径下）：
// ws 端点与 HTTP 端点都必须跟随 base，否则网关形态下资产与连接全部 404。
function resolveBasePath(): string {
  const base = typeof import.meta.env.BASE_URL === "string" ? import.meta.env.BASE_URL : "/";
  // 统一构建（desktop 的 web.html）使用相对 base "./"，浏览器实际挂载点由页面目录决定：
  // LAN 根路径 => ""（/api、/ws），子路径部署则保留该目录。独立 Web 构建仍是绝对 base。
  if (base === "./" || base === "") {
    const pathname = window.location.pathname;
    const directory = pathname.endsWith("/")
      ? pathname
      : pathname.slice(0, pathname.lastIndexOf("/") + 1);
    return directory === "/" ? "" : directory.replace(/\/+$/u, "");
  }
  return base.endsWith("/") ? base.slice(0, -1) : base;
}

function resolveDefaultWsOrigin(): string {
  return `${window.location.protocol === "https:" ? "wss:" : "ws:"}//${window.location.host}${resolveBasePath()}`;
}

async function resolveWebBootstrap(): Promise<WebBootstrapResult> {
  const params = new URLSearchParams(window.location.search);
  const remoteId = params.get("remote");
  const wsUrl = remoteId
    ? `${resolveDefaultWsOrigin()}/ws/remote/${remoteId}`
    : `${resolveDefaultWsOrigin()}/ws`;

  if (remoteId) {
    return { wsUrl };
  }

  try {
    const response = await fetch(`${resolveBasePath()}/api/server-info`, {
      cache: "no-store",
    });
    if (!response.ok) {
      return { wsUrl };
    }
    const serverInfo = (await response.json()) as Partial<ServerRemoteInfo>;
    const workspace = Array.isArray(serverInfo.workspaces) ? serverInfo.workspaces[0] : undefined;
    return {
      wsUrl,
      ...(workspace?.path ? { initialWorkspaceAbsPath: workspace.path } : {}),
      ...(workspace?.workspaceIdentity
        ? { initialWorkspaceIdentity: workspace.workspaceIdentity }
        : {}),
      supportsLanPeerController: serverInfo.capabilities?.lanPeerController === true,
    };
  } catch {
    return { wsUrl };
  }
}

function WebBootstrapErrorScreen({ message }: { message: string }) {
  return (
    <div className="h-dvh min-h-dvh w-screen bg-background text-foreground">
      <div className="mx-auto flex h-full w-full max-w-lg items-center px-4">
        <section className="w-full rounded-xl border border-card-border bg-card p-5">
          <div className="flex items-center gap-3">
            <span className="size-2 rounded-full bg-destructive" />
            <h1 className="text-ui-xs font-medium">
              {/^zh\b/i.test(navigator.language) ? "Web 启动失败" : "Web bootstrap failed"}
            </h1>
          </div>
          <p className="mt-2 break-all text-ui-xs/relaxed text-foreground-subtle">{message}</p>
          <button
            type="button"
            className="mt-4 rounded-lg border border-border bg-surface px-3 py-2 text-ui-xs text-foreground-subtle hover:bg-surface-hover"
            onClick={() => {
              window.location.reload();
            }}
          >
            {/^zh\b/i.test(navigator.language) ? "重试" : "Retry"}
          </button>
        </section>
      </div>
    </div>
  );
}

function renderWebBootstrapError(error: unknown): void {
  document.title = "Mode - Web";
  root.render(
    <WebBootstrapErrorScreen message={error instanceof Error ? error.message : String(error)} />,
  );
}

function isChineseLocale(): boolean {
  return /^zh\b/i.test(navigator.language);
}

/**
 * 浏览器/手机首次进入：用被控端远程控制页的 6 位配对码换 LAN 令牌。
 * 服务端在 /api/lan/pair 响应上写 HttpOnly Cookie，页面只负责重载。
 */
function WebLanPairingScreen({ basePath }: { basePath: string }) {
  const zh = isChineseLocale();
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const submit = async () => {
    const trimmed = code.trim().toUpperCase();
    if (!trimmed || busy) {
      return;
    }
    setBusy(true);
    setError("");
    try {
      const response = await fetch(`${basePath}/api/lan/pair`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ code: trimmed, label: "Web browser" }),
      });
      if (!response.ok) {
        setError(
          response.status === 429
            ? zh
              ? "尝试过于频繁，请稍后再试。"
              : "Too many attempts. Try again later."
            : response.status === 404
              ? zh
                ? "此 Web 服务未开启配对，请使用带 ?token= 的访问链接。"
                : "Pairing is not enabled on this Web server. Open the URL that includes ?token=."
              : zh
                ? "配对码无效或已过期，请在被控端重新生成。"
                : "The pairing code is invalid or expired. Generate a new one.",
        );
        return;
      }
      window.location.reload();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex h-dvh min-h-dvh w-screen items-center justify-center bg-background px-4 text-foreground">
      <section className="w-full max-w-md rounded-xl border border-card-border bg-card p-5">
        <h1 className="text-ui-base font-medium">
          {zh ? "连接到这台 Mode 设备" : "Connect to this Mode device"}
        </h1>
        <p className="mt-2 text-ui-xs/relaxed text-foreground-subtle">
          {zh
            ? "在桌面端「远程控制」页生成 6 位配对码，然后输入到这里。"
            : "Generate a 6-digit pairing code on the desktop Remote Control page, then enter it here."}
        </p>
        <input
          className="mt-4 w-full rounded-lg border border-border bg-surface px-3 py-2 font-mono text-ui-base tracking-[0.3em] outline-none focus:border-input-border-focused"
          value={code}
          maxLength={8}
          autoFocus
          inputMode="text"
          autoCapitalize="characters"
          placeholder="XXXXXX"
          onChange={(event) => setCode(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") void submit();
          }}
        />
        {error ? <p className="mt-3 text-ui-xs text-destructive">{error}</p> : null}
        <button
          type="button"
          className="mt-4 w-full rounded-lg border border-border bg-surface px-3 py-2 text-ui-base font-medium hover:bg-surface-hover disabled:opacity-60"
          disabled={busy || !code.trim()}
          onClick={() => void submit()}
        >
          {busy ? (zh ? "正在连接…" : "Connecting…") : zh ? "连接" : "Connect"}
        </button>
        <p className="mt-3 text-ui-xs/relaxed text-foreground-subtle">
          {zh
            ? "如果这是独立部署的 Web 服务，请使用带 ?token= 的访问链接。"
            : "For a standalone Web deployment, open the URL that includes ?token=."}
        </p>
      </section>
    </div>
  );
}

function renderWebLanPairing(basePath: string): void {
  document.title = "Mode - Web";
  root.render(<WebLanPairingScreen basePath={basePath} />);
}

/** 401 = 需要 LAN 令牌；其他状态交给正常 bootstrap 报错。 */
async function needsLanPairing(basePath: string): Promise<boolean> {
  try {
    const response = await fetch(`${basePath}/api/server-info`, { cache: "no-store" });
    return response.status === 401;
  } catch {
    return false;
  }
}

async function bootstrapWebApp() {
  const params = new URLSearchParams(window.location.search);
  if (isWebOAuthCallback(params)) {
    renderWebAuthCallbackPage();
    return;
  }

  const basePath = resolveBasePath();
  if (await needsLanPairing(basePath)) {
    renderWebLanPairing(basePath);
    return;
  }

  let bootstrap: WebBootstrapResult;
  try {
    bootstrap = await resolveWebBootstrap();
  } catch (error) {
    renderWebBootstrapError(error);
    return;
  }

  try {
    const services = await connectViaWebSocket(bootstrap.wsUrl, {
      onClose: () => {},
    });
    const platform = createWebPlatform(bootstrap.supportsLanPeerController === true);
    document.title = "Mode - Web + Server";

    root.render(
      <AppErrorBoundary>
        <ModeIntlProvider
          settingService={services.settingService}
          broadcastService={services.broadcastService}
        >
          <Root
            services={services}
            platform={platform}
            initialWorkspaceAbsPath={bootstrap.initialWorkspaceAbsPath}
            initialWorkspaceIdentity={bootstrap.initialWorkspaceIdentity}
            initialTaskId={bootstrap.initialTaskId}
            restoreSession={bootstrap.restoreSession}
            allowOpenWorkspace={bootstrap.allowOpenWorkspace}
            preferDirectoryBrowser
            supportsEmbeddedBrowser={false}
            // LAN 主控已接通（docs/specs/web-remote-workspace.md）；SSH/WSL/Docker
            // 仍由 platform.connectRemote 明确拒绝。桌面 LAN 静态站没有该主控面时
            // 保持关闭，避免把用户带进必然失败的向导。
            allowRemoteWorkspace={bootstrap.supportsLanPeerController === true}
          />
        </ModeIntlProvider>
      </AppErrorBoundary>,
    );
  } catch (error) {
    renderWebBootstrapError(error);
  }
}

void bootstrapWebApp();
