import { ServiceChannels } from "@mode/shared";
import type {
  TraceId,
  ModeAgentMcpServer,
  ModeDeliveryKind,
  ModeMessageWithParts,
  ModelSelection,
  ModePermissionRequestParams,
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
} from "@mode/shared";
import { createServiceDescriptor } from "#src/descriptors.js";

export interface ModeSessionWorkspaceTarget {
  workspacePath: string;
  workspaceIdentity?: string;
  remoteSessionId?: string;
}

export type ModeSessionReadWorkspacePresentationParams = ModeSessionWorkspaceTarget;

export interface ModeTaskTarget extends ModeSessionWorkspaceTarget {
  sessionId: string;
}

export interface ModeSessionCreateParams extends ModeSessionWorkspaceTarget {
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

export interface ModeSessionResumeParams extends ModeTaskTarget {
  model?: ModelSelection;
  thoughtLevel?: string;
  mcpServers?: ModeAgentMcpServer[];
  /**
   * 默认广播 resume 得到的历史快照，并让 shadow 订阅请求初始 snapshot。
   * 续聊发送前的 runtime 预恢复会关闭它，避免旧终态快照覆盖本地已开始的新输入运行态。
   */
  broadcastSnapshot?: boolean;
}

export interface ModeSessionListParams extends ModeSessionWorkspaceTarget {
  includeArchived?: boolean;
  limit?: number;
}

export interface ModeSessionReadParams extends ModeTaskTarget {
  deliveryKind?: ModeDeliveryKind;
  messageLimit?: number;
  afterSeq?: number;
}

export interface ModeSessionMessagesParams extends ModeTaskTarget {
  afterMessageId?: string;
  limit?: number;
}

export interface ModeSessionEventsParams extends ModeTaskTarget {
  afterSeq?: number;
  limit?: number;
}

export interface ModeSessionSetModelParams extends ModeTaskTarget {
  model: ModelSelection;
  expectedRevision?: number;
  persistAsWorkspaceLastUsed?: boolean;
}

export interface ModeSessionSetThoughtLevelParams extends ModeTaskTarget {
  thoughtLevel?: string;
  expectedRevision?: number;
  persistAsWorkspaceLastUsed?: boolean;
}

export interface ModeSessionSetModeParams extends ModeTaskTarget {
  mode: ModeSessionMode;
  expectedRevision?: number;
}

export interface ModeSessionSubscribeParams extends ModeTaskTarget {
  deliveryKind: ModeDeliveryKind;
  afterSeq?: number;
  includeSnapshot?: boolean;
  eventCoalescing?: {
    mode: "background-summary";
    intervalMs?: number;
  };
}

export type ModeSessionServiceEvent =
  | { type: "session.event"; event: ModeSessionEvent }
  | { type: "state.updated"; notification: ModeStateUpdatedNotification }
  | { type: "permission.request"; request: ModePermissionRequestParams }
  | { type: "userInput.request"; request: ModeUserInputRequestParams }
  | {
      type: "userInput.response";
      requestId: string;
      response: ModeUserInputResponse;
    }
  | { type: "snapshot"; snapshot: ModeSessionStateSnapshot };

export interface ModeSessionInitializeResult {
  available: boolean;
  workspaceKey: string;
  protocolName?: string;
  protocolVersion?: number;
  transportKind?: "stdio" | "websocket";
  reason?: string;
  reasonCode?: "provider_not_ready";
}

export interface ModeSessionWorkspaceRuntimeIdentity {
  generation: number;
  identity: string;
  processId?: number;
  workspaceKey: string;
}

export interface IModeSessionService {
  initializeWorkspace(params: ModeSessionWorkspaceTarget): Promise<ModeSessionInitializeResult>;
  getWorkspaceRuntimeIdentity(
    params: ModeSessionWorkspaceTarget,
  ): Promise<ModeSessionWorkspaceRuntimeIdentity>;
  readWorkspacePresentation(
    params: ModeSessionReadWorkspacePresentationParams,
  ): Promise<ModeWorkspacePresentation>;
  createSession(params: ModeSessionCreateParams): Promise<ModeSessionStateSnapshot>;
  resumeSession(params: ModeSessionResumeParams): Promise<ModeSessionStateSnapshot>;
  listSessions(params: ModeSessionListParams): Promise<ModeSessionInfo[]>;
  readSession(params: ModeSessionReadParams): Promise<ModeSessionStateSnapshot>;
  readSessionMessages(params: ModeSessionMessagesParams): Promise<ModeMessageWithParts[]>;
  readSessionEvents(params: ModeSessionEventsParams): Promise<ModeSessionEvent[]>;
  promoteDeferredDraftSession(params: ModeTaskTarget): Promise<void>;
  closeSession(params: ModeTaskTarget): Promise<void>;
  closeDeferredDraftSession(params: ModeTaskTarget): Promise<boolean>;
  setModel(params: ModeSessionSetModelParams): Promise<ModeSessionStateSnapshot>;
  setThoughtLevel(params: ModeSessionSetThoughtLevelParams): Promise<ModeSessionStateSnapshot>;
  setMode(params: ModeSessionSetModeParams): Promise<ModeSessionStateSnapshot>;
  // renderer 订阅面走 agentService 的 conversation/sessions-index 帧通道。
}

export const IModeSessionService = createServiceDescriptor<IModeSessionService>(
  ServiceChannels.ModeSession,
);
