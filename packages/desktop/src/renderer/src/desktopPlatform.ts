import { DesktopCommandIds, buildLocalMediaPreviewUrl, type IPlatformService } from "@mode/shared";

import { desktopBrowserPlatformBridge } from "./desktopBrowserPlatformBridge.js";

export function createDesktopPlatform(options: {
  isLocalDevelopmentRuntime: boolean;
}): IPlatformService {
  return {
    canSelectFilePath: true,
    createLocalMediaPreviewUrl: buildLocalMediaPreviewUrl,
    isLocalDevelopmentRuntime: options.isLocalDevelopmentRuntime,
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
    openUpdateStatusWindow: () => window.mode.openUpdateStatusWindow?.() ?? Promise.resolve(),
    getAutoUpdatePreferences: () =>
      window.mode.getAutoUpdatePreferences?.() ??
      Promise.resolve({ autoDownloadAndInstallUpdates: false }),
    setAutoDownloadAndInstallUpdates: (enabled) =>
      window.mode.setAutoDownloadAndInstallUpdates?.(enabled) ?? Promise.resolve(),
    getDesktopSessionActivity: () =>
      window.mode.getDesktopSessionActivity?.() ??
      Promise.resolve({ runningAgentSessionCount: 0 }),
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
