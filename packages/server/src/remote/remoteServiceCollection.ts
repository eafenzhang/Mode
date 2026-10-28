/**
 * 把远端 `IServiceAccessor` 映射成一个本地 `ServiceCollection`，供 `/ws/remote/:id`
 * 一次性暴露给浏览器（docs/specs/web-remote-workspace.md）。
 *
 * 只注册 accessor 上实际存在的服务：local-only 服务（lanAccess/cloudAccount/
 * remoteDesktop/remoteService/windowController）在远端 attachment 上通常不存在，
 * 缺失即不暴露，前端按 optional 语义降级。不要在这里伪造空实例。
 */
import { ServiceCollection, type IServiceAccessor } from "@mode/services";
import {
  IBotsService,
  IClientConfigService,
  IClientScenesService,
  ICloudAccountService,
  ICodingPlanSubscriptionService,
  ICommandsService,
  IConversationMirrorService,
  IConversationShareService,
  ICredentialService,
  ICuaPermissionService,
  IFileService,
  IFileWatcherService,
  IGitCheckpointService,
  IGitService,
  IHooksService,
  ILanAccessService,
  IMcpSyncService,
  IMediaPreviewService,
  IMemoryService,
  IModeAgentService,
  IModeSessionService,
  IModeTaskService,
  IModelSelectionService,
  IOAuthService,
  IOffPeakTaskService,
  IOnboardingRecordService,
  IOrcaRouterService,
  IPluginManagementService,
  IPluginsService,
  IPluginSyncService,
  IPromptAttachmentTransferService,
  IProviderSettingsService,
  IRemoteDesktopService,
  IRemoteService,
  ISettingService,
  ISettingsSyncService,
  ISkillSyncService,
  ISkillsService,
  ISubagentsService,
  ISystemService,
  ITerminalService,
  IUsageStatsService,
  IWindowControllerService,
} from "@mode/services";

export function createRemoteServiceCollection(accessor: IServiceAccessor): ServiceCollection {
  const services = new ServiceCollection();
  services.register(IFileService, accessor.fileService);
  if (accessor.mediaPreviewService) {
    services.register(IMediaPreviewService, accessor.mediaPreviewService);
  }
  services.register(IGitService, accessor.gitService);
  services.register(IGitCheckpointService, accessor.gitCheckpointService);
  services.register(ISystemService, accessor.systemService);
  services.register(ITerminalService, accessor.terminalService);
  services.register(ISettingService, accessor.settingService);
  if (accessor.onboardingRecordService) {
    services.register(IOnboardingRecordService, accessor.onboardingRecordService);
  }
  services.register(ICredentialService, accessor.credentialService);
  if (accessor.lanAccessService) {
    services.register(ILanAccessService, accessor.lanAccessService);
  }
  if (accessor.cloudAccountService) {
    services.register(ICloudAccountService, accessor.cloudAccountService);
  }
  if (accessor.remoteDesktopService) {
    services.register(IRemoteDesktopService, accessor.remoteDesktopService);
  }
  if (accessor.remoteService) {
    services.register(IRemoteService, accessor.remoteService);
  }
  if (accessor.windowControllerService) {
    services.register(IWindowControllerService, accessor.windowControllerService);
  }
  services.register(IBotsService, accessor.botsService);
  services.register(IFileWatcherService, accessor.fileWatcherService);
  services.register(IOAuthService, accessor.oauthService);
  if (accessor.orcaRouterService) {
    services.register(IOrcaRouterService, accessor.orcaRouterService);
  }
  services.register(IProviderSettingsService, accessor.providerSettingsService);
  services.register(IModelSelectionService, accessor.modelSelectionService);
  services.register(IUsageStatsService, accessor.usageStatsService);
  services.register(ICodingPlanSubscriptionService, accessor.codingPlanSubscriptionService);
  services.register(IClientConfigService, accessor.clientConfigService);
  services.register(IClientScenesService, accessor.clientScenesService);
  services.register(IOffPeakTaskService, accessor.offPeakTaskService);
  services.register(ISkillsService, accessor.skillsService);
  services.register(ISkillSyncService, accessor.skillSyncService);
  if (accessor.conversationMirrorService) {
    services.register(IConversationMirrorService, accessor.conversationMirrorService);
  }
  services.register(IMcpSyncService, accessor.mcpSyncService);
  services.register(IPluginSyncService, accessor.pluginSyncService);
  services.register(IPluginsService, accessor.pluginsService);
  services.register(IPluginManagementService, accessor.pluginManagementService);
  services.register(ISubagentsService, accessor.subagentsService);
  services.register(ICommandsService, accessor.commandsService);
  services.register(IHooksService, accessor.hooksService);
  services.register(IMemoryService, accessor.memoryService);
  services.register(ISettingsSyncService, accessor.settingsSyncService);
  services.register(IPromptAttachmentTransferService, accessor.promptAttachmentTransferService);
  services.register(IModeTaskService, accessor.modeTaskService);
  services.register(IModeAgentService, accessor.modeAgentService);
  services.register(IModeSessionService, accessor.modeSessionService);
  if (accessor.cuaPermissionService) {
    services.register(ICuaPermissionService, accessor.cuaPermissionService);
  }
  services.register(IConversationShareService, accessor.conversationShareService);
  return services;
}
