import { ServiceChannels } from "@zcode/shared";
import { createServiceDescriptor } from "../descriptors.js";
import type {
  ZCodeConfigOption,
  ZCodeProvider,
  BotConfig,
  BotContextState,
  BotConversationKind,
  BotConversationState,
  BotInboundMessage,
  BotOutboundMessage,
  BotProvider,
  BotProviderCallbackResult,
  BotServiceStatus,
  BotWorkspaceRef,
  BotsConfigFile,
  ZCodeAutomationBotDeliveryTarget,
} from "@zcode/shared";
import type { ZCodeAgentAppRuntimePreferences } from "../zcode-agent/zcodeAgent.js";

export interface BotCreateBindCodeParams {
  botId?: string;
  allowedWorkspaces?: string[];
  ttlMs?: number;
}

export interface BotSaveBotParams {
  bot: BotConfig;
  credentialValue?: string;
  webhookSecretValue?: string;
}

/** 对话摘要（绑定菜单选对话用）：身份 + 当前挂载的会话/工作区。 */
export interface BotConversationSummary {
  botId: string;
  conversationKey: string;
  conversationKind: BotConversationKind;
  conversationId: string;
  conversationLabel?: string;
  /** 当前绑定的桌面会话；未绑定时为 null。 */
  taskId: string | null;
  workspacePath?: string;
}

export interface BotTestResult {
  ok: boolean;
  message: string;
  name?: string;
  provider?: BotProvider;
}

export interface BotBindCodeResult {
  code: string;
  expiresAt: number;
}

export interface BotListWorkspaceRefsParams {
  currentWorkspace?: BotWorkspaceRef;
}

export interface BotUserConfigOptionsParams {
  workspacePath: string;
  workspaceIdentity?: string;
  provider: ZCodeProvider;
}

export interface BotAutomationRunWatchParams {
  target: ZCodeAutomationBotDeliveryTarget;
  taskId: string;
  workspacePath: string;
  workspaceIdentity?: string;
}

export interface BotSendMediaParams {
  botId: string;
  /** 绝对路径，或相对 bot 当前工作区的路径。 */
  filePath: string;
  caption?: string;
  /** 群聊显式指定 chatId；私聊省略时发给绑定的用户。 */
  chatId?: string;
}

export interface BotWorkspaceBindingParams {
  botId: string;
  workspacePath: string;
  workspaceIdentity?: string;
}

export interface BotWorkspaceBindingInfo {
  /** 绑定到该工作区的机器人（一个工作区可绑定多个）。 */
  botIds: string[];
}

/** UI 焦点变化：工作区切换 / 会话切换时由 renderer 通知，绑定到该工作区的 bot 跟随。 */
export interface BotUiFocusParams {
  workspacePath: string;
  workspaceIdentity?: string;
  /** 会话焦点通知携带目标 taskId；工作区焦点通知可省略（由 UI 侧活跃任务决定） */
  taskId?: string | null;
}

export interface BotWeixinRegistrationBeginResult {
  qrCode: string;
  qrUrl: string;
  interval: number;
  expiresAt: number;
}

export interface BotWeixinRegistrationPollParams {
  qrCode: string;
}

export interface BotWecomRegistrationPollParams {
  scode: string;
}

export type BotWeixinRegistrationPollResult =
  | {
      status: "pending" | "scanned";
      interval: number;
    }
  | {
      status: "success";
      botToken: string;
      botId?: string;
    }
  | {
      status: "expired" | "error";
      message?: string;
    };

export interface BotWecomRegistrationBeginResult {
  /** 轮询凭据（一次性） */
  scode: string;
  /** 渲染成二维码的授权地址 */
  authUrl: string;
  interval: number;
  expiresAt: number;
}

export type BotWecomRegistrationPollResult =
  | { status: "pending" | "scanned"; interval: number }
  | {
      status: "success";
      /** 智能机器人 botid */
      botId: string;
      /** 机器人 secret（保存到 credentialRef） */
      secret: string;
    }
  | { status: "expired" | "cancelled" | "denied" | "error"; message?: string };

export interface BotFeishuRegistrationBeginParams {
  domain?: "feishu" | "lark";
}

export interface BotFeishuRegistrationBeginResult {
  deviceCode: string;
  qrUrl: string;
  userCode: string;
  interval: number;
  expiresAt: number;
  domain: "feishu" | "lark";
  pollDomain?: "feishu" | "lark";
}

export interface BotFeishuRegistrationPollParams {
  deviceCode: string;
  domain?: "feishu" | "lark";
  pollDomain?: "feishu" | "lark";
}

export type BotFeishuRegistrationPollResult =
  | {
      status: "pending";
      interval: number;
      domain: "feishu" | "lark";
      pollDomain?: "feishu" | "lark";
    }
  | {
      status: "success";
      appId: string;
      appSecret: string;
      domain: "feishu" | "lark";
      appName?: string;
      openId?: string;
    }
  | {
      status: "access_denied" | "expired" | "error";
      message?: string;
      domain: "feishu" | "lark";
    };

export interface IBotsService {
  /**
   * 将 App 全局交互偏好同步给 Bot 已持有的远端 runtime；不得为此建立新的远端连接。
   */
  syncAppRuntimePreferences(preferences: ZCodeAgentAppRuntimePreferences): Promise<void>;
  getStatus(): Promise<BotServiceStatus>;
  getConfig(): Promise<BotsConfigFile>;
  listWorkspaceRefs(params?: BotListWorkspaceRefsParams): Promise<BotWorkspaceRef[]>;
  getUserConfigOptions(params: BotUserConfigOptionsParams): Promise<ZCodeConfigOption[]>;
  /** 把 bot 绑定为某个工作区的专属 bot；绑定后 bot 上下文钉定该工作区，禁用 /workspace 切换。 */
  bindBotToWorkspace(params: BotWorkspaceBindingParams): Promise<void>;
  /** 解除工作区绑定；bot 恢复 allowedWorkspaces 自由切换。 */
  unbindBotFromWorkspace(params: BotWorkspaceBindingParams): Promise<void>;
  getWorkspaceBotBinding(params: Omit<BotWorkspaceBindingParams, "botId">): Promise<BotWorkspaceBindingInfo>;
  /** 该 bot 当前绑定的全部工作区；空数组表示未绑定（访问范围由用户自行管理）。 */
  listBotWorkspaceBindings(params: { botId: string }): Promise<BotWorkspaceRef[]>;
  /**
   * UI 会话/工作区焦点同步。只影响绑定到该工作区的 bot（未绑定的 bot 不被 UI 劫持）。
   * 目标上下文已一致时幂等短路，这也是 UI↔bot 双向联动的防循环核心。
   */
  notifyUiSessionFocus(params: BotUiFocusParams): Promise<void>;
  beginFeishuRegistration(
    params?: BotFeishuRegistrationBeginParams,
  ): Promise<BotFeishuRegistrationBeginResult>;
  pollFeishuRegistration(
    params: BotFeishuRegistrationPollParams,
  ): Promise<BotFeishuRegistrationPollResult>;
  beginWeixinRegistration(): Promise<BotWeixinRegistrationBeginResult>;
  pollWeixinRegistration(
    params: BotWeixinRegistrationPollParams,
  ): Promise<BotWeixinRegistrationPollResult>;
  /** 企业微信智能机器人一键创建：生成扫码授权，扫码后返回 botId + secret。 */
  beginWecomRegistration(): Promise<BotWecomRegistrationBeginResult>;
  pollWecomRegistration(
    params: BotWecomRegistrationPollParams,
  ): Promise<BotWecomRegistrationPollResult>;
  saveConfig(config: BotsConfigFile): Promise<BotsConfigFile>;
  listBots(): Promise<BotConfig[]>;
  saveBot(params: BotSaveBotParams): Promise<BotConfig>;
  removeBotSecret(botId: string): Promise<BotConfig>;
  deleteBot(botId: string): Promise<void>;
  testBot(botId: string): Promise<BotTestResult>;
  createBindCode(params: BotCreateBindCodeParams): Promise<BotBindCodeResult>;
  getBotStates(): Promise<BotConversationState[]>;
  resetBotState(contextKey: string): Promise<void>;
  /**
   * 把受信目录（当前工作区 / 应用数据目录 / 系统临时目录）内的文件作为媒体发到会话。
   * 路径越界、文件不存在或超过平台大小上限都会抛错。
   */
  sendBotMedia(params: BotSendMediaParams): Promise<void>;
  /** 当前被机器人绑定的任务列表（桌面任务行的绿点标识用）。 */
  listBotTaskBindings(): Promise<
    Array<{
      botId: string;
      provider: BotProvider;
      conversationKey: string;
      conversationKind: BotConversationKind;
      conversationId: string;
      conversationLabel?: string;
      workspacePath: string;
      workspaceIdentity?: string;
      taskId: string;
    }>
  >;
  /** 把机器人绑定到指定桌面会话（会话右键菜单入口）。 */
  bindBotToTask(params: {
    botId: string;
    /** 绑定哪个 IM 对话到该会话；省略时用该 bot 当前唯一/最近活跃的对话。 */
    conversationKey?: string;
    workspacePath: string;
    workspaceIdentity?: string;
    taskId: string;
  }): Promise<{ ok: boolean; reason?: "busy" | "missing" | "workspace" | "conversation" }>;
  /** 解除机器人与任务的绑定（可按对话粒度解绑；省略对话键则解绑该 bot 指向此任务的全部对话）。 */
  unbindBotFromTask(params: {
    botId: string;
    taskId: string;
    conversationKey?: string;
  }): Promise<void>;
  /** 该 bot 的对话列表（含是否绑定桌面会话），供绑定菜单选择对话。 */
  listBotConversations(params: { botId: string }): Promise<BotConversationSummary[]>;
  /** 桌面输入镜像：绑定会话里的桌面 prompt 转发到 IM（双向实时同步的桌面→IM 方向）。 */
  notifyDesktopUserMessage(params: {
    taskId: string;
    workspacePath: string;
    workspaceIdentity?: string;
    text: string;
  }): Promise<void>;

  /**
   * 桌面端发送前的镜像武装：为绑定到该 task 的对话挂上助手回复的流订阅。
   * 流订阅只从武装那一刻开始收事件（且每轮终态会释放），因此必须在提示进入 Agent 之前调用；
   * 提问回显仍走 notifyDesktopUserMessage（ACK 之后），避免被拒的发送在 IM 侧留下幽灵提问。
   */
  armConversationReplyMirror(params: {
    taskId: string;
    workspacePath: string;
    workspaceIdentity?: string;
  }): Promise<void>;

  /** 在 automation prompt 派发前订阅终态，并把结果回推到创建它的 Bot 会话。 */
  watchAutomationRun(params: BotAutomationRunWatchParams): Promise<void>;
  handleInboundMessage(message: BotInboundMessage): Promise<BotOutboundMessage[]>;
  handleProviderCallback(provider: BotProvider, payload: unknown): Promise<BotOutboundMessage[]>;
  handleProviderCallbackResponse(
    provider: BotProvider,
    payload: unknown,
  ): Promise<BotProviderCallbackResult>;
}

export const IBotsService = createServiceDescriptor<IBotsService>(ServiceChannels.Bots);
