export type {
  FileBinaryPreview,
  FileEntry,
  FileMediaPreview,
  FileStat,
  FileTextSlice,
  FileWatchEvent,
  WorkspaceFileEntry,
  SystemInfo,
  AppSettings,
  ElectronReleaseChannel,
  IntegratedTerminalShellDialect,
  IntegratedTerminalShellOption,
  IntegratedTerminalShellSelection,
  Locale,
  LocalePreference,
  ModeInteractionBehavior,
  TabId,
  TabState,
  ResourceUsageCategory,
  ResourceUsageBaseGroupKey,
  ResourceUsageProcess,
  HostResourceUsageProcess,
  ResourceUsageSnapshot,
  RemoteTargetSnapshot,
  RemoteWorkspaceSessionEntry,
  PersistedWorkspaceSessionEntry,
} from "./protocol.js";
export type { WorkspacePurpose } from "./workspacePurpose.js";
export { DEFAULT_LOCALE } from "./protocol.js";
export { MODE_VERSION, MODE_COMMIT, MODE_BUILD_TIME } from "./version.js";
export type { HelloMessage, HelloAckMessage } from "./handshake.js";
export type { ModeEnv, ModeProductFlavor } from "./env.js";
export type { RemoteAssetInstallMode } from "./remoteAssetInstallMode.js";
export type {
  RemoteResourcePackageId,
  RemoteResourcePackageSelection,
} from "./remoteResourcePackages.js";
export type {
  DockerConnectOptions,
  LanConnectOptions,
  RemoteTarget,
  SSHConnectOptions,
  WSLConnectOptions,
} from "./remoteTarget.js";
export { stripRemoteTargetSecrets } from "./remoteTarget.js";
export { buildSshRemoteHostKey } from "./remoteSshHostKey.js";
export { buildRemoteEnvironmentKey } from "./remoteEnvironmentKey.js";
export type {
  ShortcutChannel,
  ShortcutCommandEntry,
  ShortcutCommandId,
  ParsedShortcutBinding,
} from "./shortcutCommands.js";
export {
  SHORTCUT_COMMANDS,
  getDefaultShortcutBindings,
  isValidShortcutBinding,
  normalizeShortcutKey,
  parseShortcutBinding,
  serializeShortcutBinding,
} from "./shortcutCommands.js";
export {
  MODE_ENV,
  MODE_PRODUCT_FLAVOR,
  MODE_APP_VERSION_ENV,
  MODE_BUILD_COMMIT_ID_ENV,
  RUNTIME_MODE_DEBUG,
  normalizeModeEnv,
  normalizeModeProductFlavor,
} from "./env.js";
export * from "./errors.js";
export type { SessionCreateSource } from "./sessionCreateSource.js";
export { resolveSafeEndpointHostname } from "./endpointHostname.js";
export * from "./validation.js";
export * from "./api.js";
export * from "./mode-protocol/index.js";
export * from "./account-provider-state.js";
// re-home：旧协议承重面的幸存文件（消费者继续走 barrel，零感知）
export * from "./mode-protocol-legacy-types.js";
export * from "./mode-task-types-core.js";
export * from "./task-realtime-core.js";
export * from "./remote-workspace-identity.js";
export * from "./mode-api-retry-status.js";
export * from "./mode-network-debug-status.js";
export * from "./mode-session-visible-content.js";
export * from "./official-mcp-auth.js";
export * from "./official-mcp-tool-error.js";
export * from "./conversation-message-projection-policy.js";
export * from "./conversation-share.js";
export * from "./conversation-mirror.js";
export * from "./officialPlatformPolicy.js";
export * from "./conversation-preview-artifacts.js";
export * from "./mode-session-task-status.js";
export * from "./mode-tool-projection-memory.js";
export * from "./mode-slash-command-help.js";
export * from "./modeEndpoint.js";
export * from "./mode-source-headers.js";
export * from "./mode-agent-policy.js";
export * from "./mode-media-policy.js";
export * from "./media-preview.js";
export * from "./plugin-display-name.js";
export * from "./mode-agent-runtime.js";
export * from "./runtimeEnv.js";
export * from "./dynamic-workflow-feature.js";
export * from "./markdown-artifact-images.js";
export * from "./serviceAuthority.js";
export * from "./server-remote.js";

export interface ICredentialStore {
  get(key: string): Promise<string | null>;
  set(key: string, value: string): Promise<void>;
  delete(key: string): Promise<void>;
}

export * from "./test-ids.js";
export * from "./test-ids-workflow.js";
export * from "./channels.js";
export * from "./storage.js";
export * from "./oauth.js";
export * from "./desktopMenu.js";
export * from "./e2e-test-bridge.js";
export * from "./remoteAppConfig.js";
export * from "./helpAppConfig.js";
export * from "./remoteAssetInstallMode.js";
export * from "./onboardingRecord.js";
export * from "./remoteResourcePackages.js";
export * from "./plan-identity.js";
export {
  BROWSER_SCREENSHOT_SURFACE_PREPARE_TIMEOUT_MS,
  BROWSER_VIEW_RESTORE_BOOTSTRAP_URL,
  LOCAL_MEDIA_PREVIEW_SCHEME,
  DesktopCommandIds,
  buildLocalMediaPreviewUrl,
  createOpenInEditorRemoteTarget,
} from "./platform.js";
export type { LaunchMarks } from "./launchMarks.js";
export { LAUNCH_MARKS_QUERY_KEY, parseLaunchMarks, serializeLaunchMarks } from "./launchMarks.js";
export type {
  CancelPendingRemoteConnectionRequest,
  BindRemoteWorkspaceSessionContextRequest,
  BotRemoteWorkspaceReconnectedEvent,
  BrowserTabResidencyState,
  BrowserViewCloseTabNotification,
  BrowserViewCloseTabRequest,
  BrowserViewOperationPayload,
  BrowserViewResidencyReportPayload,
  BrowserViewResidencyTransitionPayload,
  BrowserViewRestoredTabShell,
  BrowserViewRestoreTabsRequest,
  BrowserViewScreenshotSurfacePreparePayload,
  BrowserViewScreenshotSurfaceReadyPayload,
  BrowserViewScreenshotSurfaceReleasePayload,
  BrowserViewViewportChangedPayload,
  ChromeBrowserDataImportError,
  ChromeBrowserDataImportOptions,
  ChromeBrowserDataImportResult,
  ConnectRemoteRequest,
  CreateTempTextAttachmentRequest,
  CreateTempTextAttachmentResult,
  SaveFileRequest,
  SaveFileResult,
  PrintPageToPdfResult,
  DesktopCommandId,
  CuaOsSupport,
  DesktopWindowChromeState,
  DesktopTitleBarTheme,
  DockerContainerInfo,
  EditorInfo,
  ApplicationIconInfo,
  ApplicationIconLocator,
  ApplicationIconRequest,
  BrowserGuestAttachRejectReason,
  BrowserGuestAttachResult,
  EmbeddedBrowserDataClearResult,
  EmbeddedBrowserOpenUrlRequest,
  IPlatformService,
  OpenInEditorRemoteTarget,
  OpenInEditorOptions,
  PostUpdateReleaseNotesPayload,
  RemoteConnectionRuntimeLog,
  RemoteSessionClosedEvent,
  RemoteServiceSession,
  SSHConfigAliasOption,
  TaskNotificationPayload,
  UpdateCheckResultPayload,
  UpdateStatePayload,
  WSLDistro,
  ModeStdioTapDevState,
} from "./platform.js";
export type {
  CuaAccessibilitySettingsResult,
  CuaPermissionKind,
  OpenCuaPermissionOnboardingOptions,
  PrepareCuaHelperPermissionDragResult,
} from "./cuaAccessibilitySettings.js";
export type { ModeTaskCreateResult } from "./mode-task-types.js";
export * from "./mode-task-types.js";
export * from "./automation-types.js";
export * from "./off-peak-types.js";
export * from "./background-task-control-merge.js";
export * from "./background-task-controls.js";
export * from "./background-task-notifications.js";
export * from "./background-bash-jobs.js";
export * from "./mode-agent-model-state.js";
export * from "./task-realtime.js";
export { formatTimestamp, formatLogPrefix } from "./log-format.js";
export * from "./model-provider-types.js";
export * from "./model-provider-family.js";
export * from "./provider-family-connection-selection.js";
export * from "./provider-provisioning.js";
export * from "./custom-model-value.js";
export * from "./model-selection-types.js";
export * from "./model-selection-key.js";
export * from "./model-selection.js";
export * from "./legacy-model-provider-identity.js";
export * from "./official-glm-model-id.js";
export * from "./skills-types.js";
export * from "./skill-sync.js";
export * from "./mcp-sync.js";
export * from "./plugin-sync.js";
export * from "./remote-sync.js";
export * from "./plugin-types.js";
export * from "./subagents-types.js";
export * from "./settings-source.js";
export * from "./settings-errors.js";
export * from "./app-runtime-preferences.js";
export * from "./command-types.js";
export * from "./plugin-marketplaces.js";
export * from "./lineChangeStat.js";
export * from "./process-names.js";
export * from "./mcp.js";
export * from "./runtime-tool-runtime.js";
export * from "./git.js";
export * from "./bots.js";
export * from "./assistant-message-parts.js";
export * from "./modePersistedMessageMerge.js";
export * from "./assistant-presentation.js";
export * from "./tool-call-summary.js";
export * from "./tool-identity.js";
export * from "./streaming-tool-input-preview.js";
export * from "./tool-plan-adapter.js";
export * from "./permission-request-preview.js";
export * from "./settings-sync.js";
export * from "./uuid.js";
export * from "./usage-stats.js";
export * from "./coding-plan-subscription.js";
export * from "./forceUpdate.js";
export * from "./intranetProbe.js";
export * from "./intranetDefaults.js";
export * from "./hooks.js";
export * from "./openrouter-attribution.js";
export * from "./orcarouter.js";
export * from "./workspaceSessionRestore.js";
export * from "./skill-scan-policy.js";
export * from "./browser-use/index.js";

export * from "./coding-plan-reset.js";
export {
  parseSubagentMarkdownSelection,
  formatSubagentMarkdownModel,
} from "./subagent-markdown-selection.js";
export * from "./memoryDiagnostics.js";
export * from "./database-startup.js";
export * from "./processResourceSample.js";
export * from "./execution-state.js";

export { bashOutputDisplaySchema } from "./bash-output-display.js";

export * from "./localTtft.js";
export * from "./pluginStoreOrder.js";
export * from "./clientConfig.js";
export * from "./pluginStoreOrdering.js";
export * from "./session-debug.js";

export * from "./remoteWorkspaceConnection.js";
export * from "./lanAccess.js";
export * from "./data-root.js";
export * from "./data-root-decision.js";
export * from "./env-names.js";

// --- 上游 v3.15.1 并入所需的最小再导出补丁（自动生成，逐符号显式导出，避免 export * 撞名）---
export { type ChannelContentPart, type ChannelReplyHostRequest, type ChannelReplyResult, channelContentText, channelMentionSchema, channelReplyHostRequestSchema, channelReplyRequestSchema, resolveChannelReplyParts } from './channel-mention.js';
export { type CloudContentBundle, type CloudContentPayload, contentBundleSchema } from './cloudContent.js';
export { RequestVerificationProof } from './coding-plan-subscription.js';
export { type CreateFeedbackTicketInput, DEFAULT_FEEDBACK_TICKET_FRAMEWORK, type FeedbackAttachment, type FeedbackAttachmentKind, type FeedbackComment, type FeedbackCommentAttachment, type FeedbackDeviceInfo, type FeedbackListQuery, type FeedbackListResult, type FeedbackTicketDetail, type FeedbackTicketEvent, type FeedbackTicketEventType, type FeedbackTicketFramework, type FeedbackTicketModule, type FeedbackTicketSeverity, type FeedbackTicketStatus, type FeedbackTicketSummary, type FeedbackTicketType } from './feedback.js';
export { redactFeedbackText } from './feedbackPrivacy.js';
export { HIGHSPEED_CARD_EXPIRED_PROVIDER_ERROR_CODE, type HighspeedCardApi, type HighspeedCardSnapshot, type HighspeedCardUsage, type HighspeedDrawRequest, type HighspeedDrawResponse, type HighspeedMessageMeta, type HighspeedMockScenario, type HighspeedPrepareFallbackReason, type HighspeedPrepareTurnParams, type HighspeedPrepareTurnResult, type HighspeedServiceSnapshot, type HighspeedTurnExecution, highspeedDrawResponseSchema, highspeedHealthyResponseSchema, resolveHighspeedDrawProviderId, resolveHighspeedProviderId } from './highspeed.js';
export { type MarketingAction, type MarketingAsset, type MarketingDelivery, type MarketingPopup, type MarketingTouchSnapshot, type MarketingVisual, marketingAssetSchema, parseMarketingTouchResponse } from './marketingTouch.js';
export { type NetworkCaptureBatch, type NetworkCaptureBridge, type NetworkCaptureSnapshot, networkCaptureBatchSchema } from './networkCapture.js';
export { type OutputStyle, type OutputStyleConfig } from './output-style.js';
export { type ProcessResourceRuntimeSurface } from './processResourceTelemetry.js';
export { ProjectAccessTokenClient } from './project-access-token-client.js';
export { ProjectAccessTokenTransientError } from './project-access-token.js';
export { resolveWorkspaceTelemetryDetail } from './remoteUsageTelemetry.js';
export { RENDERER_ACTION_TRACE_MAX_BATCH_BYTES, RENDERER_ACTION_TRACE_MAX_BATCH_SPANS, type RendererActionTraceAttributes, type RendererActionTraceBatchV1, type RendererActionTraceConfigV1, type RendererActionTraceGroup, type RendererActionTraceResourceV1, type RendererActionTraceSpanV1 } from './rendererActionTrace.js';
export { allowRequestVerificationHeaders, requestVerificationInteractionAvailable, requiresRequestVerification } from './request-security-edition/verification-policy.js';
export { createRewardsInjectionScript } from './rewardsBridge.js';
export { REWARDS_PARTITION, buildRewardsUrl, isTrustedRewardsUrl } from './rewardsEmbedded.js';
export { type AutomationSessionCreateTelemetry, type SessionCreateClientKind } from './sessionCreateTelemetry.js';
export { parseAgentProfileFromMarkdown } from './subagent-profile-parser.js';
export { type SubagentRuntimeState, resolveSubagentRuntimeProfiles, subagentRuntimeConfigSchema } from './subagent-runtime-config.js';
export { type ArmsCustomEventPayload, type TelemetryEventPayload, type TelemetryRendererContext, collectTelemetryRendererContext, sanitizeTelemetryErrorMessage, sanitizeTelemetryEventDetail } from './telemetry.js';
export { redactTelemetryText, resolveTelemetryModelId, resolveTelemetryProviderScope, sanitizeTelemetryModelValue } from './telemetryRedaction.js';
export { WEB_REMOTE_CONTROL_HEARTBEAT_ACK_TIMEOUT_MS, WEB_REMOTE_CONTROL_HEARTBEAT_INTERVAL_MS, WEB_REMOTE_CONTROL_RECONNECT_JITTER_MS, getWebRemoteControlHeartbeatDelayMs, getWebRemoteControlHeartbeatJitterMs, getWebRemoteControlReconnectJitterMs } from './web-remote-control-heartbeat.js';
export { WEB_REMOTE_CONTROL_RPC_TRANSPORT_LIMITS, WebRemoteControlRpcTransportAssembler, WebRemoteControlRpcTransportEncodingError, type WebRemoteControlRpcTransportFault, type WebRemoteControlRpcTransportFramePayload, type WebRemoteControlRpcTransportIdentity, type WebRemoteControlRpcTransportPayload, encodeWebRemoteControlRpcTransportMessage, measureWebRemoteControlRpcRelayEnvelopeBytes, parseWebRemoteControlRpcTransportPayload } from './web-remote-control-rpc-transport.js';
export { type WebRemoteControlAppPayload, type WebRemoteControlExternalWorkspaceBridge, type WebRemoteControlFailure, type WebRemoteControlFailureReason, type WebRemoteControlMobileDeviceInfo, type WebRemoteControlMobileNavigationIntent, type WebRemoteControlRelayDataMessage, type WebRemoteControlRelayIncomingMessage, type WebRemoteControlRelayOutgoingMessage, type WebRemoteControlRelayRole, type WebRemoteControlRpcFramePayload, type WebRemoteControlStatus, type WebRemoteControlTaskDisplayStatus, type WebRemoteControlTaskTarget, type WebRemoteControlThemeSeed, type WebRemoteControlWindowBootstrapResult, type WebRemoteControlWorkspaceBridge, type WebRemoteControlWorkspaceListResult, type WebRemoteControlWorkspaceTarget, isWebRemoteControlThemeSeed, resolveWebRemoteControlRoutePathFromBaseUrl, resolveWebRemoteControlWorkspaceKey } from './web-remote-control.js';
export { BotGroupInputSource, BotTopicHistoryBatch, BotTopicMessage, TOPIC_RESOURCE_RELAY_CHANNEL, TOPIC_RESOURCE_RELAY_CHUNK_BYTES, type ModeMcpReadResourceResult, type ModeMcpUiAppToolAcceptedResult, type ModeMcpUiCallToolResult, type ModeMcpUiCancelCallResult, type ModeMcpUiListResourceTemplatesResult, type ModeMcpUiListResourcesResult, type ModeMcpUiReadResourceResult, type ModeMcpUiRegisterAppToolsResult, type ModeMcpUiUnregisterAppToolsResult, type ModePluginUiSurface, type ModePluginsListUiSurfacesResult, type ModeTopicResourceReadParams, type ModeTopicResourceReadResult, type ModeTopicResourceRelayRequest, type ModeWorkflowsForRunCandidate, modeMcpReadResourceResultSchema, modeMcpUiAppToolAcceptedResultSchema, modeMcpUiCallToolResultSchema, modeMcpUiCancelCallResultSchema, modeMcpUiCloseInstanceResultSchema, modeMcpUiListResourceTemplatesResultSchema, modeMcpUiListResourcesResultSchema, modeMcpUiOpenInstanceResultSchema, modeMcpUiReadResourceResultSchema, modeMcpUiRegisterAppToolsResultSchema, modeMcpUiResourceSubscriptionResultSchema, modeMcpUiSamplingResultSchema, modeMcpUiUnregisterAppToolsResultSchema, modeSubagentsReadRuntimeConfigParamsSchema, modeTopicResourceCancelParamsSchema, modeTopicResourceReadParamsSchema, modeTopicResourceReadResultSchema, modeTopicResourceRelayChunkRequestSchema, modeTopicResourceRelayChunkSchema, modeTopicResourceRelayKeySchema, modeTopicResourceRelayMetadataSchema, modeTopicResourceRelayRequestSchema } from './mode-protocol/index.js';
export { type ModeSessionContentProfile } from './mode-session-content-profile.js';
export { type ModeCodexConnectivityCheckResult, type ModeSwitchAgentResult, type ModeSyncTaskSessionBindingResult } from './mode-task-types.js';
