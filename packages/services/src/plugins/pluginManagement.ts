// 平台能力面收敛：设置页「插件管理」的薄服务接口。
//
// 背景：pluginManagementStore / usePluginUninstall 过去直接注入 IModeAgentService，
// UI 层因此散布 13 个 plugins/* 旧协议词的消费点。收敛为独立薄 service 后，UI 只依赖
// 本接口；plugins/* 词表的 host 侧消费点收拢到 pluginManagementService 一处（插件的
// 事实源在 mode-cli 进程，服务实现仍经 agent 协议往返——plugins 词表的收口归属
// 插件能力面自身的协议演进，不在会话 v4 词表范围内）。
// 注意与既有 IPluginsService（已 retired 的 marketplace pluginStore 通道）区分：
// 那套接口按 pluginName+marketplace 寻址且方法语义过时，不复用避免签名冲突。
import type { Event } from "@mode/rpc";
import type {
  ModePluginOperationProgressNotification,
  ModePluginsConfigureResult,
  ModePluginsCancelOperationResult,
  ModePluginsDescribeResult,
  ModePluginsInstallResult,
  ModePluginsListResult,
  ModePluginsMarketplaceMutationResult,
  ModePluginsOverviewResult,
  ModePluginsReferenceCatalogResult,
  ModePluginsRestoreBuiltinResult,
  ModePluginsSetEnabledResult,
  ModePluginsUninstallResult,
  ModePluginsValidateResult,
} from "@mode/shared";
import { ServiceChannels } from "@mode/shared";
import { createServiceDescriptor } from "../descriptors.js";
import type {
  ModeAgentAddPluginMarketplaceParams,
  ModeAgentConfigurePluginParams,
  ModeAgentCancelPluginOperationParams,
  ModeAgentDescribePluginParams,
  ModeAgentInstallPluginParams,
  ModeAgentPluginReferenceCatalogParams,
  ModeAgentResolveSuggestedPluginReferenceParams,
  ModeAgentResetPluginConfigParams,
  ModeAgentPluginViewParams,
  ModeAgentRemovePluginMarketplaceParams,
  ModeAgentRestoreBuiltinPluginParams,
  ModeAgentSetPluginEnabledParams,
  ModeAgentUninstallPluginParams,
  ModeAgentUpdatePluginMarketplaceParams,
  ModeAgentUpdatePluginParams,
  ModeAgentValidatePluginParams,
} from "../mode-agent/modeAgentPluginParams.js";

export interface IPluginManagementService {
  listPlugins(params: ModeAgentPluginViewParams): Promise<ModePluginsListResult>;
  /**
   * Plugin 对话引用 catalog：
   * 带 sessionId → session-owned 冻结 catalog；不带 → workspace 当前 catalog。
   * 实现路由到 workspace 级 agent client，不走插件管理独立进程。
   */
  getPluginReferenceCatalog(
    params: ModeAgentPluginReferenceCatalogParams,
  ): Promise<ModePluginsReferenceCatalogResult>;
  resolveSuggestedPluginReference(
    params: ModeAgentResolveSuggestedPluginReferenceParams,
  ): Promise<import("@mode/shared").ModePluginsResolveSuggestedReferenceResult>;
  onDynamicPluginOperationProgress(
    operationId: string,
  ): Event<ModePluginOperationProgressNotification>;
  getPluginsOverview(params: ModeAgentPluginViewParams): Promise<ModePluginsOverviewResult>;
  addPluginMarketplace(
    params: ModeAgentAddPluginMarketplaceParams,
  ): Promise<ModePluginsMarketplaceMutationResult>;
  removePluginMarketplace(
    params: ModeAgentRemovePluginMarketplaceParams,
  ): Promise<ModePluginsMarketplaceMutationResult>;
  updatePluginMarketplace(
    params: ModeAgentUpdatePluginMarketplaceParams,
  ): Promise<ModePluginsMarketplaceMutationResult>;
  installPlugin(params: ModeAgentInstallPluginParams): Promise<ModePluginsInstallResult>;
  cancelPluginOperation(
    params: ModeAgentCancelPluginOperationParams,
  ): Promise<ModePluginsCancelOperationResult>;
  uninstallPlugin(params: ModeAgentUninstallPluginParams): Promise<ModePluginsUninstallResult>;
  updatePlugin(params: ModeAgentUpdatePluginParams): Promise<ModePluginsInstallResult>;
  restoreBuiltinPlugin(
    params: ModeAgentRestoreBuiltinPluginParams,
  ): Promise<ModePluginsRestoreBuiltinResult>;
  configurePlugin(params: ModeAgentConfigurePluginParams): Promise<ModePluginsConfigureResult>;
  resetPluginConfig(
    params: ModeAgentResetPluginConfigParams,
  ): Promise<ModePluginsConfigureResult>;
  validatePlugin(params: ModeAgentValidatePluginParams): Promise<ModePluginsValidateResult>;
  describePlugin(params: ModeAgentDescribePluginParams): Promise<ModePluginsDescribeResult>;
  setPluginEnabled(params: ModeAgentSetPluginEnabledParams): Promise<ModePluginsSetEnabledResult>;
}

export const IPluginManagementService = createServiceDescriptor<IPluginManagementService>(
  ServiceChannels.PluginManagement,
);
