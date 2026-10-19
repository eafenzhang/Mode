import { DesktopCommandIds, buildLocalMediaPreviewUrl, type IPlatformService } from "@mode/shared";
import type { RemoteDesktopSignalMessage } from "@mode/shared/remote-desktop";

import { desktopBrowserPlatformBridge } from "./desktopBrowserPlatformBridge.js";

/** Tauri 壳的产品窗口同时注入这两个全局；Electron 两者都没有。 */
function isTauriRendererRuntime(): boolean {
  return (
    typeof globalThis !== "undefined" &&
    ("__TAURI__" in globalThis || "__TAURI_INTERNALS__" in globalThis)
  );
}

export function createDesktopPlatform(options: {
  isLocalDevelopmentRuntime: boolean;
}): IPlatformService {
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
    selectFiles: () => window.mode.selectFiles?.() ?? Promise.resolve([]),
    openDataRootImport: () => window.mode.openDataRootImport?.(),
    createTempTextAttachment: (payload) => window.mode.createTempTextAttachment(payload),
    onRemoteConnectionLog: (handler) => window.mode.onRemoteConnectionLog(handler),
    onRemoteSessionClosed: (handler) => window.mode.onRemoteSessionClosed(handler),
    onBotRemoteWorkspaceReconnected: (handler) =>
      window.mode.onBotRemoteWorkspaceReconnected(handler),
    activateOrSetWorkspace: (path) =>
      window.mode.activateOrSetWorkspace?.(path) ?? Promise.resolve({ activated: false }),
    connectRemote: (remoteOptions, requestId, context) =>
      window.mode.connectRemote(remoteOptions, requestId, context),
    cancelPendingRemoteConnection: (requestId) =>
      window.mode.cancelPendingRemoteConnection?.(requestId) ?? Promise.resolve(),
    bindRemoteWorkspaceSessionContext: (context) =>
      window.mode.bindRemoteWorkspaceSessionContext?.(context) ?? Promise.resolve(),
    disposeRemoteSession: (sessionId) => window.mode.disposeRemoteSession(sessionId),
    isDockerAvailable: () => window.mode.isDockerAvailable(),
    listWSLDistros: () => window.mode.listWSLDistros(),
    listDockerContainers: () => window.mode.listDockerContainers(),
    listSSHConfigAliases: () => window.mode.listSSHConfigAliases(),
    getLanAccessState: () => window.mode.getLanAccessState(),
    setLanAccessEnabled: (enabled) => window.mode.setLanAccessEnabled(enabled),
    createLanAccessPairCode: () => window.mode.createLanAccessPairCode(),
    removeLanAccessClient: (clientId) => window.mode.removeLanAccessClient(clientId),
    resetLanAccessTokens: () => window.mode.resetLanAccessTokens(),
    discoverLanPeers: () => window.mode.discoverLanPeers(),
    pairLanPeer: (request) => window.mode.pairLanPeer(request),
    getLanPairedPeers: () => window.mode.getLanPairedPeers(),
    getLanPeerWorkspaces: (serverId) => window.mode.getLanPeerWorkspaces(serverId),
    removeLanPairedPeer: (serverId) => window.mode.removeLanPairedPeer(serverId),
    renameLanPairedPeer: (serverId, name) => window.mode.renameLanPairedPeer(serverId, name),
    // 局域网远程桌面（main 的 rd/* IPC；通道名与形状冻结在 main/remoteDesktop/ipcChannels.ts）。
    rdGetPeerCapability: (serverId) => window.mode.rdGetPeerCapability(serverId),
    rdStartSession: (serverId, quality, codec) =>
      window.mode.rdStartSession(serverId, quality, codec),
    rdGetControlledState: () => window.mode.rdGetControlledState(),
    // 信令是高频小包，直接透传给 main；失败只记日志不抛出，避免打断媒体面。
    rdSendSignal: (message: RemoteDesktopSignalMessage) => {
      void window.mode.rdSendSignal(message).catch(() => undefined);
    },
    rdEndSession: (reason) => window.mode.rdEndSession(reason),
    rdReadClipboardText: () => window.mode.rdReadClipboardText(),
    rdGetServerState: () => window.mode.rdGetServerState(),
    rdSetServerEnabled: (enabled) => window.mode.rdSetServerEnabled(enabled),
    rdRevokeClient: (clientId) => window.mode.rdRevokeClient(clientId),
    rdAuthorizeClient: (clientId) => window.mode.rdAuthorizeClient(clientId),
    onRemoteDesktopSessionEvent: (handler) => window.mode.onRemoteDesktopSessionEvent(handler),
    loadMcpFromUserDirectory: (payload) => window.mode.loadMcpFromUserDirectory(payload),
    saveMcpToUserDirectory: (payload) => window.mode.saveMcpToUserDirectory(payload),
    migrateLegacyCommonMcp: (payload) => window.mode.migrateLegacyCommonMcp(payload),
    openExternal: (url) => window.mode.openExternal(url),
    openCommunity: () => window.mode.executeDesktopCommand(DesktopCommandIds.OpenCommunity),
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
          if (normalized.startsWith("fa")) return "fa-IR" as const;
          return "en-US" as const;
        })(),
      ),
    setTitleBarTheme: (theme) => window.mode.setTitleBarTheme(theme),
    getDeviceId: () =>
      (window as Window & { __MODE_DEVICE_ID__?: string }).__MODE_DEVICE_ID__ ?? "",
    // 共享平台协议仍要求这两个方法；审计版不采集、不转发，避免业务 hook 调用失败。
  };
}
