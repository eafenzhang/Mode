import type {
  ModeAgentMcpServer,
  ModeAutomationScheduleRule,
  ModeMcpListMode,
  ModelSelection,
} from "@mode/shared";

export interface ModeAgentWorkspaceTarget {
  workspacePath: string;
  workspaceIdentity?: string;
  /** 远程 workspace 的运行时会话身份；只用于隔离/路由，不能替代 workspacePath。 */
  remoteSessionId?: string;
}

export interface ModeAgentPluginViewParams extends ModeAgentWorkspaceTarget {
  configScope?: "user" | "workspace";
}

export interface ModeAgentListMcpServerStatusesParams extends ModeAgentWorkspaceTarget {
  mcpServers?: ModeAgentMcpServer[];
  mode?: ModeMcpListMode;
}

export interface ModeAgentAddPluginMarketplaceParams extends ModeAgentWorkspaceTarget {
  dryRun?: boolean;
  operationId?: string;
  source: string;
}

export interface ModeAgentRemovePluginMarketplaceParams extends ModeAgentWorkspaceTarget {
  marketplace: string;
}

export interface ModeAgentUpdatePluginMarketplaceParams extends ModeAgentWorkspaceTarget {
  marketplace?: string;
  operationId?: string;
}

export interface ModeAgentInstallPluginParams extends ModeAgentWorkspaceTarget {
  dryRun?: boolean;
  marketplace: string;
  operationId?: string;
  pluginName: string;
  scope?: "user" | "workspace";
}

export interface ModeAgentCancelPluginOperationParams {
  operationId: string;
}

export interface ModeAgentUninstallPluginParams extends ModeAgentWorkspaceTarget {
  marketplace?: string;
  pluginId?: string;
  pluginName?: string;
  removeCache?: boolean;
}

export interface ModeAgentUpdatePluginParams extends ModeAgentWorkspaceTarget {
  pluginId?: string;
  marketplace?: string;
}

export interface ModeAgentRestoreBuiltinPluginParams extends ModeAgentWorkspaceTarget {
  pluginId: string;
}

export interface ModeAgentConfigurePluginParams extends ModeAgentWorkspaceTarget {
  clearOptionKeys?: string[];
  dryRun?: boolean;
  options: Record<string, unknown>;
  pluginId: string;
  scope?: "user" | "workspace";
}

export interface ModeAgentResetPluginConfigParams extends ModeAgentWorkspaceTarget {
  pluginId: string;
  scope?: "user" | "workspace";
}

export interface ModeAgentValidatePluginParams extends ModeAgentWorkspaceTarget {
  marketplace?: string;
  pluginName?: string;
  source?: string;
}

export interface ModeAgentDescribePluginParams extends ModeAgentWorkspaceTarget {
  marketplace: string;
  pluginName: string;
}

export interface ModeAgentSetPluginEnabledParams extends ModeAgentWorkspaceTarget {
  enabled: boolean;
  operationId?: string;
  pluginId: string;
  scope?: "user" | "workspace";
}

// Plugin 对话引用 catalog：
// 带 sessionId → session-owned 冻结 catalog（必须路由到持有该 session 的 workspace client）；
// 不带 → workspace 当前 catalog（新建草稿 Picker）。
export interface ModeAgentPluginReferenceCatalogParams extends ModeAgentWorkspaceTarget {
  sessionId?: string;
}

// Composer Skill catalog：与 Plugin 引用相同，以 sessionId 区分 workspace 当前目录和
// resident Session runtime 快照；不参与 Settings 管理目录。
export interface ModeAgentSkillReferenceCatalogParams extends ModeAgentWorkspaceTarget {
  sessionId?: string;
}
export interface ModeAgentResolveSuggestedPluginReferenceParams extends ModeAgentWorkspaceTarget {
  stableId: string;
  operationId: string;
  clientMode: "desktop-continuous" | "web-remote-replayable";
  deliveryKind: "desktop-continuous" | "web-remote-replayable";
}

// ---- 定时任务(automation)管理参数 ----

export interface ModeAgentCreateAutomationParams extends ModeAgentWorkspaceTarget {
  title: string;
  cronExpr: string;
  relativeDelayMinutes?: number;
  prompt: string;
  modelSelection?: ModelSelection;
  mode?: string;
  recurring?: boolean;
  maxRuns?: number;
  endAt?: number;
  scheduleRule?: ModeAutomationScheduleRule;
}

export interface ModeAgentUpdateAutomationParams extends ModeAgentWorkspaceTarget {
  automationId: string;
  title?: string;
  cronExpr?: string;
  prompt?: string;
  modelSelection?: ModelSelection | null;
  mode?: string | null;
  recurring?: boolean;
  maxRuns?: number | null;
  endAt?: number | null;
  scheduleRule?: ModeAutomationScheduleRule | null;
  scheduleEditedByUser?: boolean;
}

export interface ModeAgentAutomationIdParams extends ModeAgentWorkspaceTarget {
  automationId: string;
}

export interface ModeAgentSetAutomationEnabledParams extends ModeAgentWorkspaceTarget {
  automationId: string;
  enabled: boolean;
}

export interface ModeAgentDeleteAutomationRunParams extends ModeAgentWorkspaceTarget {
  runId: string;
}
