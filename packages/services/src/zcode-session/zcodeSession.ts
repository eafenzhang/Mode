import { ServiceChannels } from "@zcode/shared";
import type {
  TraceId,
  ModeAgentMcpServer,
  ModeDeliveryKind,
  ModeMessageWithParts,
  ModelSelection,
  ModePermissionRequestParams,
  ModeProviderRuntimeHeadersRequestParams,
  ModeUserInputRequestParams,
  ModeUserInputResponse,
  ModeSessionInfo,
  ModeSessionImportHistory,
  ModeSessionEvent,
  ModeSessionMode,
  ModeSessionPersistence,
  ModeSessionStateSnapshot,
  ModeStateUpdatedNotification,
  ModeWorkspacePresentation,
} from "@zcode/shared";
import { createServiceDescriptor } from "#src/descriptors.js";
import type { ProviderRuntimeHeadersUiResponse } from "#src/zcode-agent/zcodeAgent.js";

export interface ZCodeSessionWorkspaceTarget {
  workspacePath: string;
  workspaceIdentity?: string;
  remoteSessionId?: string;
}

export type ZCodeSessionReadWorkspacePresentationParams = ZCodeSessionWorkspaceTarget;

export interface ZCodeTaskTarget extends ZCodeSessionWorkspaceTarget {
  sessionId: string;
}

export interface ZCodeSessionCreateParams extends ZCodeSessionWorkspaceTarget {
  /** 仅导入事务使用的预分配 ID；普通新会话继续由 Agent 分配。 */
  sessionId?: string;
  sessionTraceId?: TraceId;
  parentSessionId?: string;
  mode?: ModeSessionMode;
  model?: ModelSelection;
  persistence?: ModeSessionPersistence;
  thoughtLevel?: string;
  mcpServers?: ModeAgentMcpServer[];
  importedHistory?: ModeSessionImportHistory;
}

export interface ZCodeSessionResumeParams extends ZCodeTaskTarget {
  model?: ModelSelection;
  thoughtLevel?: string;
  mcpServers?: ModeAgentMcpServer[];
  /**
   * 默认广播 resume 得到的历史快照，并让 shadow 订阅请求初始 snapshot。
   * 续聊发送前的 runtime 预恢复会关闭它，避免旧终态快照覆盖本地已开始的新输入运行态。
   */
  broadcastSnapshot?: boolean;
}

export interface ZCodeSessionListParams extends ZCodeSessionWorkspaceTarget {
  includeArchived?: boolean;
  limit?: number;
}

export interface ZCodeSessionReadParams extends ZCodeTaskTarget {
  deliveryKind?: ModeDeliveryKind;
  messageLimit?: number;
  afterSeq?: number;
}

export interface ZCodeSessionMessagesParams extends ZCodeTaskTarget {
  afterMessageId?: string;
  limit?: number;
}

export interface ZCodeSessionEventsParams extends ZCodeTaskTarget {
  afterSeq?: number;
  limit?: number;
}

export interface ZCodeSessionSetModelParams extends ZCodeTaskTarget {
  model: ModelSelection;
  expectedRevision?: number;
  persistAsWorkspaceLastUsed?: boolean;
}

export interface ZCodeSessionSetThoughtLevelParams extends ZCodeTaskTarget {
  thoughtLevel?: string;
  expectedRevision?: number;
  persistAsWorkspaceLastUsed?: boolean;
}

export interface ZCodeSessionSetModeParams extends ZCodeTaskTarget {
  mode: ModeSessionMode;
  expectedRevision?: number;
}

export interface ZCodeSessionRespondProviderRuntimeHeadersParams extends ZCodeTaskTarget {
  requestId: string;
  response: ProviderRuntimeHeadersUiResponse;
}

export interface ZCodeSessionSubscribeParams extends ZCodeTaskTarget {
  deliveryKind: ModeDeliveryKind;
  afterSeq?: number;
  includeSnapshot?: boolean;
  eventCoalescing?: {
    mode: "background-summary";
    intervalMs?: number;
  };
}

export type ZCodeSessionServiceEvent =
  | { type: "session.event"; event: ModeSessionEvent }
  | { type: "state.updated"; notification: ModeStateUpdatedNotification }
  | { type: "permission.request"; request: ModePermissionRequestParams }
  | { type: "userInput.request"; request: ModeUserInputRequestParams }
  | {
      type: "userInput.response";
      requestId: string;
      response: ModeUserInputResponse;
    }
  | { type: "providerRuntimeHeaders.request"; request: ModeProviderRuntimeHeadersRequestParams }
  | { type: "snapshot"; snapshot: ModeSessionStateSnapshot };

export interface ZCodeSessionInitializeResult {
  available: boolean;
  workspaceKey: string;
  protocolName?: string;
  protocolVersion?: number;
  transportKind?: "stdio" | "websocket";
  reason?: string;
  reasonCode?: "provider_not_ready";
}

export interface ZCodeSessionWorkspaceRuntimeIdentity {
  generation: number;
  identity: string;
  processId?: number;
  workspaceKey: string;
}

export interface IZCodeSessionService {
  initializeWorkspace(params: ZCodeSessionWorkspaceTarget): Promise<ZCodeSessionInitializeResult>;
  getWorkspaceRuntimeIdentity(
    params: ZCodeSessionWorkspaceTarget,
  ): Promise<ZCodeSessionWorkspaceRuntimeIdentity>;
  readWorkspacePresentation(
    params: ZCodeSessionReadWorkspacePresentationParams,
  ): Promise<ModeWorkspacePresentation>;
  createSession(params: ZCodeSessionCreateParams): Promise<ModeSessionStateSnapshot>;
  resumeSession(params: ZCodeSessionResumeParams): Promise<ModeSessionStateSnapshot>;
  listSessions(params: ZCodeSessionListParams): Promise<ModeSessionInfo[]>;
  readSession(params: ZCodeSessionReadParams): Promise<ModeSessionStateSnapshot>;
  readSessionMessages(params: ZCodeSessionMessagesParams): Promise<ModeMessageWithParts[]>;
  readSessionEvents(params: ZCodeSessionEventsParams): Promise<ModeSessionEvent[]>;
  promoteDeferredDraftSession(params: ZCodeTaskTarget): Promise<void>;
  closeSession(params: ZCodeTaskTarget): Promise<void>;
  closeDeferredDraftSession(params: ZCodeTaskTarget): Promise<boolean>;
  setModel(params: ZCodeSessionSetModelParams): Promise<ModeSessionStateSnapshot>;
  setThoughtLevel(params: ZCodeSessionSetThoughtLevelParams): Promise<ModeSessionStateSnapshot>;
  setMode(params: ZCodeSessionSetModeParams): Promise<ModeSessionStateSnapshot>;
  respondProviderRuntimeHeaders(
    params: ZCodeSessionRespondProviderRuntimeHeadersParams,
  ): Promise<void>;
  // M5 删波次 2：onDynamicSessionEvent（旧 session/subscribe 词表的 renderer 订阅面）
  // 已删除——v4 UI 走 agentService 的 conversation/sessions-index 帧通道，
  // 该方法在 renderer 侧已无任何调用者（grep packages/ui packages/desktop 为零）。
}

export const IZCodeSessionService = createServiceDescriptor<IZCodeSessionService>(
  ServiceChannels.ZCodeSession,
);
