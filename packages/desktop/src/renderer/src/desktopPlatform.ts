import { buildLocalMediaPreviewUrl, type IPlatformService } from "@mode/shared";
import type { RemoteDesktopSignalMessage } from "@mode/shared/remote-desktop";
import type {
  ILanAccessService,
  IRemoteDesktopService,
  IRemoteService,
  IServiceAccessor,
} from "@mode/services";

import { desktopBrowserPlatformBridge } from "./desktopBrowserPlatformBridge.js";
import { createDesktopRemotePlatformBridge } from "./desktopRemotePlatformBridge.js";
import { resolveSelectFilesCapability } from "./platformSelectFiles.js";

/** Tauri 壳的产品窗口同时注入这两个全局；Electron 两者都没有。 */
export function isTauriRendererRuntime(): boolean {
  return (
    typeof globalThis !== "undefined" &&
    ("__TAURI__" in globalThis || "__TAURI_INTERNALS__" in globalThis)
  );
}

export function createDesktopPlatform(options: {
  isLocalDevelopmentRuntime: boolean;
  /**
   * 当前窗口 Host 的服务面。`MessagePort` 在 `createDesktopPlatform` 之后才送达，
   * 所以必须是 getter：模块加载期捕获一次会把委托永久锁死在 fallback。
   */
  getHostServices?: () => IServiceAccessor | null;
}): IPlatformService {
  /**
   * LAN 管理面只在 Tauri 产品壳里走 Host 服务；Electron 线的写入路径保持原样（DR-LR-5）。
   * 旧 Tauri Host 没有该频道时 `lanAccessService` 代理会以 RPC 错误拒绝，而不是静默回退——
   * 回退到 `window.mode` 会变成一个被拒绝的 Promise，语义上同样是「能力不可用」。
   */
  const resolveLanAccessService = (): ILanAccessService | undefined =>
    isTauriRendererRuntime() ? options.getHostServices?.()?.lanAccessService : undefined;

  /** 远程桌面被控设置面：与 LAN 同一委托策略（LR-2）。 */
  const resolveRemoteDesktopService = (): IRemoteDesktopService | undefined =>
    isTauriRendererRuntime() ? options.getHostServices?.()?.remoteDesktopService : undefined;

  /** 远程工作区管理面（LR-4）。 */
  const resolveRemoteService = (): IRemoteService | undefined =>
    isTauriRendererRuntime() ? options.getHostServices?.()?.remoteService : undefined;

  return {
    canSelectFilePath: true,
    createLocalMediaPreviewUrl: buildLocalMediaPreviewUrl,
    isLocalDevelopmentRuntime: options.isLocalDevelopmentRuntime,
    // Tauri 的 window.mode 对 MCP 目录读写仍是 not_implemented_until_m3 桩；
    // 本地 workspace 改走 host 的 mcpSyncService，避免 MCP 设置页永久停在加载中。
    supportsLocalMcpDirectory: !isTauriRendererRuntime(),
    // Tauri 骨架期没有资源管理器窗口（Electron 主进程能力），帮助菜单据此隐藏入口。
    supportsResourceManager: !isTauriRendererRuntime(),
    selectDirectory: () => window.mode.selectDirectory(),
    selectFile: () => window.mode.selectFile(),
    // 多选只在壳真的实现时暴露；空实现会让 openAttachmentPicker 静默停在空数组。
    ...resolveSelectFilesCapability(window.mode.selectFiles),
    // Tauri 注入层没有 openDataRootImport（数据目录导入窗是旧线 main 能力）：
    // 按方法存在做能力声明，缺失时设置页同键隐藏入口，而不是留一个点了没反应的死按钮。
    ...(window.mode.openDataRootImport
      ? { openDataRootImport: () => window.mode.openDataRootImport!() }
      : {}),
    createTempTextAttachment: (payload) => window.mode.createTempTextAttachment(payload),
    activateOrSetWorkspace: (path) =>
      window.mode.activateOrSetWorkspace?.(path) ?? Promise.resolve({ activated: false }),
    // 远程工作区（LR-4）：connect/bind/dispose 与事件订阅集中在 helper 里。
    ...createDesktopRemotePlatformBridge({ getRemoteService: resolveRemoteService }),
    // 窗口行为类设置（关闭隐藏到托盘）同步给壳：壳是唯一执行者且不读业务存储。
    isDockerAvailable: () => window.mode.isDockerAvailable(),
    listWSLDistros: () => window.mode.listWSLDistros(),
    listDockerContainers: () => window.mode.listDockerContainers(),
    listSSHConfigAliases: () => window.mode.listSSHConfigAliases(),
    // LAN 管理面（LR-1）：Tauri 产品壳委托 Host 的 local-only `lan-access` 频道；
    // Electron 仍走既有 main IPC。两条路径互斥，不做双写。
    getLanAccessState: () => {
      const lan = resolveLanAccessService();
      return lan ? lan.getState() : window.mode.getLanAccessState();
    },
    setLanAccessEnabled: (enabled) => {
      const lan = resolveLanAccessService();
      return lan ? lan.setEnabled({ enabled }) : window.mode.setLanAccessEnabled(enabled);
    },
    createLanAccessPairCode: () => {
      const lan = resolveLanAccessService();
      return lan ? lan.createPairCode() : window.mode.createLanAccessPairCode();
    },
    removeLanAccessClient: (clientId) => {
      const lan = resolveLanAccessService();
      return lan ? lan.removeClient({ clientId }) : window.mode.removeLanAccessClient(clientId);
    },
    resetLanAccessTokens: () => {
      const lan = resolveLanAccessService();
      return lan ? lan.resetTokens() : window.mode.resetLanAccessTokens();
    },
    discoverLanPeers: () => {
      const lan = resolveLanAccessService();
      return lan ? lan.discoverPeers() : window.mode.discoverLanPeers();
    },
    pairLanPeer: (request) => {
      const lan = resolveLanAccessService();
      return lan ? lan.pairPeer(request) : window.mode.pairLanPeer(request);
    },
    getLanPairedPeers: () => {
      const lan = resolveLanAccessService();
      return lan ? lan.listPairedPeers() : window.mode.getLanPairedPeers();
    },
    getLanPeerWorkspaces: (serverId) => {
      const lan = resolveLanAccessService();
      return lan ? lan.getPeerWorkspaces({ serverId }) : window.mode.getLanPeerWorkspaces(serverId);
    },
    // 这两个在 Tauri 注入层曾是静默 no-op（「看着像成功」）。有 Host 服务时必须真写凭据；
    // Tauri 下拿不到 Host 服务则显式拒绝，绝不回落成 no-op。Electron 仍走既有 IPC。
    removeLanPairedPeer: (serverId) => {
      const lan = resolveLanAccessService();
      if (lan) {
        return lan.removePairedPeer({ serverId });
      }
      return isTauriRendererRuntime()
        ? Promise.reject(new Error("lan-access Host 服务不可用，无法删除配对"))
        : window.mode.removeLanPairedPeer(serverId);
    },
    renameLanPairedPeer: (serverId, name) => {
      const lan = resolveLanAccessService();
      if (lan) {
        return lan.renamePairedPeer({ serverId, name });
      }
      return isTauriRendererRuntime()
        ? Promise.reject(new Error("lan-access Host 服务不可用，无法重命名配对"))
        : window.mode.renameLanPairedPeer(serverId, name);
    },
    // 局域网远程桌面（main 的 rd/* IPC；通道名与形状冻结在 main/remoteDesktop/ipcChannels.ts）。
    // 设置面（LR-2）与主控会话（LR-3）在 Tauri 下走 Host；剪贴板读取仍留在壳（LR-3 缺口）。
    rdGetPeerCapability: (serverId) => {
      const rd = resolveRemoteDesktopService();
      return rd ? rd.getPeerCapability({ serverId }) : window.mode.rdGetPeerCapability(serverId);
    },
    rdStartSession: (serverId, quality, codec) => {
      const rd = resolveRemoteDesktopService();
      return rd
        ? rd.startSession({ serverId, quality, codec })
        : window.mode.rdStartSession(serverId, quality, codec);
    },
    rdGetControlledState: () => {
      const rd = resolveRemoteDesktopService();
      return rd ? rd.getControlledState() : window.mode.rdGetControlledState();
    },
    // 信令是高频小包；失败只记日志不抛出，避免打断媒体面。
    rdSendSignal: (message: RemoteDesktopSignalMessage) => {
      const rd = resolveRemoteDesktopService();
      if (rd) {
        void rd.sendSignal({ message }).catch(() => undefined);
        return;
      }
      void window.mode.rdSendSignal(message).catch(() => undefined);
    },
    rdEndSession: (reason) => {
      const rd = resolveRemoteDesktopService();
      return rd ? rd.endSession({ reason }) : window.mode.rdEndSession(reason);
    },
    rdReadClipboardText: () => window.mode.rdReadClipboardText(),
    rdGetServerState: () => {
      const rd = resolveRemoteDesktopService();
      return rd ? rd.getServerState() : window.mode.rdGetServerState();
    },
    rdSetServerEnabled: (enabled) => {
      const rd = resolveRemoteDesktopService();
      return rd ? rd.setServerEnabled({ enabled }) : window.mode.rdSetServerEnabled(enabled);
    },
    rdRevokeClient: (clientId) => {
      const rd = resolveRemoteDesktopService();
      return rd ? rd.revokeClient({ clientId }) : window.mode.rdRevokeClient(clientId);
    },
    rdAuthorizeClient: (clientId) => {
      const rd = resolveRemoteDesktopService();
      return rd ? rd.authorizeClient({ clientId }) : window.mode.rdAuthorizeClient(clientId);
    },
    onRemoteDesktopSessionEvent: (handler) => {
      const rd = resolveRemoteDesktopService();
      if (!rd) {
        return window.mode.onRemoteDesktopSessionEvent(handler);
      }
      // 非幂等信令：Host 只发给当前订阅者，不重放旧帧。
      const disposable = rd.onDynamicSessionEvent()(handler);
      return () => disposable.dispose();
    },
    // 被控同意窗：Tauri 用 renderer 模态（Electron 由 main 的原生窗处理，不暴露这两个方法）。
    ...(isTauriRendererRuntime()
      ? {
          onRemoteDesktopConsentRequest: (
            handler: (request: { clientId: string; label: string }) => void,
          ) => {
            const rd = resolveRemoteDesktopService();
            if (!rd) {
              return () => {};
            }
            const disposable = rd.onConsentRequest(handler);
            return () => disposable.dispose();
          },
          respondRemoteDesktopConsent: async (request: {
            clientId: string;
            decision: "allow" | "deny";
            remember: boolean;
            reason?: "user" | "timeout";
          }) => {
            const rd = resolveRemoteDesktopService();
            if (!rd) {
              throw new Error("remote-desktop Host 服务不可用");
            }
            await rd.respondConsent(request);
          },
          onRemoteDesktopControlledStateChanged: (
            handler: (state: { controlled: boolean }) => void,
          ) => {
            const rd = resolveRemoteDesktopService();
            if (!rd) {
              return () => {};
            }
            const disposable = rd.onDidChangeControlledState(handler);
            return () => disposable.dispose();
          },
        }
      : {}),
    // 这三个在 IPlatformService 里是可选的。两条注入面（旧 preload / Tauri shim）都提供了实现，
    // 但可选链会把「平台不支持」伪装成 undefined 返回值，所以这里保留「缺了就抛」的语义，
    // 同时把存在性判断显式写出来：非空断言在注入面缺失时只会抛 TypeError 且无法归因。
    loadMcpFromUserDirectory: (payload) => {
      const call = window.mode?.loadMcpFromUserDirectory;
      if (!call) throw new Error("window.mode.loadMcpFromUserDirectory is not available");
      return call(payload);
    },
    saveMcpToUserDirectory: (payload) => {
      const call = window.mode?.saveMcpToUserDirectory;
      if (!call) throw new Error("window.mode.saveMcpToUserDirectory is not available");
      return call(payload);
    },
    migrateLegacyCommonMcp: (payload) => {
      const call = window.mode?.migrateLegacyCommonMcp;
      if (!call) throw new Error("window.mode.migrateLegacyCommonMcp is not available");
      return call(payload);
    },
    openExternal: (url) => window.mode.openExternal(url),
    // 社群入口在 LR-5 从拒绝桩升级为注入层真实现（spec §10.7），这里恢复直通委托。
    openCommunity: () => window.mode.openCommunity(),
    canOpenCommunity: (locale) => window.mode.canOpenCommunity(locale),
    openInFileManager: (path) => window.mode.openInFileManager(path),
    openExternalFile: (path) => window.mode.openExternalFile(path),
    openCuaPermissionOnboarding: window.mode.openCuaPermissionOnboarding
      ? (permissionOptions) =>
          window.mode.openCuaPermissionOnboarding?.(permissionOptions) ??
          Promise.resolve({ success: false, error: "not_supported" })
      : undefined,
    prepareCuaHelperPermissionDrag: window.mode.prepareCuaHelperPermissionDrag
      ? () =>
          window.mode.prepareCuaHelperPermissionDrag?.() ??
          Promise.resolve({ success: false, error: "not_supported" })
      : undefined,
    startCuaHelperPermissionDrag: window.mode.startCuaHelperPermissionDrag
      ? () => window.mode.startCuaHelperPermissionDrag?.()
      : undefined,
    registerOAuthState: (payload) => window.mode.registerOAuthState(payload),
    onOAuthCallback: (callback) => window.mode.onOAuthCallback(callback),
    onPaymentCallback: (callback) => window.mode.onPaymentCallback(callback),
    onShareImport: (callback) => window.mode.onShareImport?.(callback) ?? (() => {}),
    notifyRendererReady: () => window.mode.notifyRendererReady(),
    showTaskNotification: (payload) => window.mode.showTaskNotification(payload),
    syncWindowTabs: (paths) => window.mode.syncWindowTabs(paths),
    syncWindowUnreadCount: (count) => window.mode.syncWindowUnreadCount(count),
    syncActiveTaskSession: (sessionId) => window.mode.syncActiveTaskSession(sessionId),
    syncAppSettings: (patch) => window.mode.syncAppSettings?.(patch),
    setShortcutRecordingActive: (active) => window.mode.setShortcutRecordingActive?.(active),
    onFocusTab: (handler) => window.mode.onFocusTab(handler),
    onNewTab: (handler) => window.mode.onNewTab(handler),
    onCloseActiveContextRequest: (handler) =>
      window.mode.onCloseActiveContextRequest?.(handler) ?? (() => {}),
    onOpenBrowserUrl: (handler) => window.mode.onOpenBrowserUrl?.(handler) ?? (() => {}),
    onBrowserViewScreenshotSurfacePrepare: (handler) =>
      window.mode.onBrowserViewScreenshotSurfacePrepare?.(handler) ?? (() => {}),
    onBrowserViewScreenshotSurfaceRelease: (handler) =>
      window.mode.onBrowserViewScreenshotSurfaceRelease?.(handler) ?? (() => {}),
    browserViewScreenshotSurfaceReady: (payload) =>
      window.mode.browserViewScreenshotSurfaceReady?.(payload),
    ...desktopBrowserPlatformBridge,
    onNewTask: (handler) => window.mode.onNewTask(handler),
    onOpenWorkspace: (handler) => {
      // 开发态或升级后的旧窗口可能仍运行未暴露 onOpenWorkspace 的 preload，
      // renderer 直接调用会在启动时崩溃。这里和 activateOrSetWorkspace 一样做兼容兜底，
      // 缺少该 bridge 时只禁用原生菜单回调，不影响应用继续打开。
      return window.mode.onOpenWorkspace?.(handler) ?? (() => {});
    },
    onOpenWorkspacePath: (handler) => window.mode.onOpenWorkspacePath?.(handler) ?? (() => {}),
    onWindowFullscreenChanged: (handler) => window.mode.onWindowFullscreenChanged(handler),
    getDesktopWindowChromeState: window.mode.getDesktopWindowChromeState
      ? () => window.mode.getDesktopWindowChromeState!()
      : undefined,
    onDesktopWindowChromeStateChanged: window.mode.onDesktopWindowChromeStateChanged
      ? (handler) => window.mode.onDesktopWindowChromeStateChanged!(handler)
      : undefined,
    getWindowControlsOverlayMetrics: () => window.mode.getWindowControlsOverlayMetrics?.() ?? null,
    onWindowControlsOverlayChanged: (handler) =>
      window.mode.onWindowControlsOverlayChanged?.(handler) ?? (() => {}),
    getDesktopZoomLevel: () =>
      window.mode.getDesktopZoomLevel?.() ?? Promise.resolve({ zoomLevel: 0 }),
    onDesktopZoomLevelChanged: (handler) =>
      window.mode.onDesktopZoomLevelChanged?.(handler) ?? (() => {}),
    onTaskNotificationClick: (handler) => window.mode.onTaskNotificationClick(handler),
    exportLogs: () => window.mode.exportLogs(),
    onUpdateReady: (callback) => window.mode.onUpdateReady(callback),
    onUpdateCheckResult: (callback) => window.mode.onUpdateCheckResult(callback),
    onUpdateStateChanged: (callback) => window.mode.onUpdateStateChanged?.(callback) ?? (() => {}),
    getUpdateState: () =>
      window.mode.getUpdateState?.() ?? Promise.resolve({ kind: "idle", enabled: true }),
    downloadUpdate: () => window.mode.downloadUpdate?.() ?? Promise.resolve(),
    cancelUpdateDownload: () => window.mode.cancelUpdateDownload?.() ?? Promise.resolve(),
    openUpdateStatusWindow: window.mode.openUpdateStatusWindow
      ? () => window.mode.openUpdateStatusWindow!()
      : undefined,
    getAutoUpdatePreferences: () =>
      window.mode.getAutoUpdatePreferences?.() ??
      Promise.resolve({ autoDownloadAndInstallUpdates: false }),
    setAutoDownloadAndInstallUpdates: (enabled) =>
      window.mode.setAutoDownloadAndInstallUpdates?.(enabled) ?? Promise.resolve(),
    getDesktopSessionActivity: () =>
      window.mode.getDesktopSessionActivity?.() ?? Promise.resolve({ runningAgentSessionCount: 0 }),
    getModeStdioTapDevState: () =>
      window.mode.getModeStdioTapDevState?.() ??
      Promise.resolve({ enabled: false, visible: false, logDir: "", statePath: "" }),
    onSettingsChanged: (callback) => window.mode.onSettingsChanged?.(callback) ?? (() => {}),
    onApplicationLocaleChanged: (callback) =>
      window.mode.onApplicationLocaleChanged?.(callback) ?? (() => {}),
    onPostUpdateReleaseNotes: (callback) => window.mode.onPostUpdateReleaseNotes(callback),
    acknowledgePostUpdateReleaseNotes: (version) =>
      window.mode.acknowledgePostUpdateReleaseNotes(version),
    skipUpdateVersion: (version) => window.mode.skipUpdateVersion?.(version) ?? Promise.resolve(),
    quitAndInstallUpdate: () => window.mode.quitAndInstallUpdate(),
    getInstalledEditors: () => window.mode.getInstalledEditors(),
    getApplicationIcon: (bundleId) =>
      window.mode.getApplicationIcon?.(bundleId) ?? Promise.resolve(null),
    openInEditor: (editorId, path, editorOptions) =>
      window.mode.openInEditor(editorId, path, editorOptions),
    executeDesktopCommand: (command) => window.mode.executeDesktopCommand(command),
    setApplicationLocale: (locale) => window.mode.setApplicationLocale(locale),
    getSystemLocale: () =>
      window.mode.getSystemLocale?.() ??
      Promise.resolve(
        (() => {
          const normalized = navigator.language.toLowerCase();
          if (normalized.startsWith("zh")) return "zh-CN" as const;
          return "en-US" as const;
        })(),
      ),
    setTitleBarTheme: (theme) => window.mode.setTitleBarTheme(theme),
    getDeviceId: () =>
      (window as Window & { __MODE_DEVICE_ID__?: string }).__MODE_DEVICE_ID__ ?? "",
    // 共享平台协议仍要求这两个方法；审计版不采集、不转发，避免业务 hook 调用失败。
  };
}
