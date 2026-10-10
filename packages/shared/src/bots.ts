/* oxlint-disable eslint(max-lines) -- Bot 共享合约集中维护 provider、状态和 schema，保持类型与校验就近。 */
import { z } from "zod";
import { modelSelectionSchema, type ModelSelection } from "./model-selection.js";
import { MODE_AGENT_PROVIDER, MODE_AGENT_PROVIDER_LABEL } from "./mode-agent-policy.js";
import type {
  ModeConfigOption,
  ModeElicitationRequest,
  ModeElicitationQuestion,
  ModePermissionRequest,
  ModePromptAttachment,
  ModeProvider,
  ModeStreamEvent,
  ModeTaskMeta,
  ModeTaskRuntimeStatus,
} from "./mode-task-types-core.js";
import {
  modeInteractionRequestOriginSchema,
  modePermissionResponseSchema,
  type ModeInteractionRequestOrigin,
  type ModePermissionResponse,
} from "./mode-protocol-legacy-types.js";
import type { Locale } from "./protocol.js";

export const botProviders = [
  "telegram",
  "webhook",
  "feishu",
  "lark",
  "weixin",
  "dingtalk",
  "discord",
  "wecom",
  // 已下线通道：AstrBot 桥接整体移除。字面量只为解析历史 bot-config 文件保留，
  // 读取配置时会连同记录一起丢弃（见 RETIRED_BOT_PROVIDERS），产品里不再可达。
  "astrbot",
] as const;

export type BotProvider = (typeof botProviders)[number];
export type FeishuBotProvider = Extract<BotProvider, "feishu" | "lark">;

/**
 * 定时任务完成后的 Bot 回推目标。只保留未来仍稳定的会话地址；当前消息 id/context token
 * 属于一次入站交互，不能持久化后复用。该字段由 Host 注入，模型工具参数不直接暴露。
 */
export const modeAutomationBotDeliveryTargetSchema = z
  .object({
    // 具备主动推送能力的平台都可接收自动化终态回推：
    // 飞书/Lark/微信（原有）+ Telegram（sendMessage）+ 企业微信（aibot_send_msg）。
    provider: z.enum(["feishu", "lark", "weixin", "telegram", "wecom"]),
    botId: z.string().trim().min(1),
    providerUserId: z.string().trim().min(1),
    chatType: z.enum(["private", "group"]),
  })
  .strict();

export type ModeAutomationBotDeliveryTarget = z.infer<
  typeof modeAutomationBotDeliveryTargetSchema
>;

export function isFeishuBotProvider(provider: BotProvider): provider is FeishuBotProvider {
  return provider === "feishu" || provider === "lark";
}
export type BotContextMode = "draft" | "task";
export type BotReplyGranularity =
  | "assistant_changes"
  | "assistant_toolcalls_changes"
  | "summary_changes"
  | "streaming_card";

export const BOT_REPLY_GRANULARITIES = [
  "assistant_changes",
  "assistant_toolcalls_changes",
  "summary_changes",
  "streaming_card",
] as const satisfies readonly BotReplyGranularity[];

export const ALL_BOT_WORKSPACES = "*";
// 绑定要在另一台设备/应用里操作（打开 IM、找到机器人、发送 /bind），
// 30 秒会让用户刚切过去就过期；5 分钟覆盖完整的手动路径，同时保留单次使用语义。
export const BOT_BIND_CODE_TTL_MS = 5 * 60_000;

export interface BotWorkspaceRef {
  id: string;
  label: string;
  workspacePath: string;
  workspaceIdentity?: string;
}

export interface BotAllowedCommands {
  status: boolean;
  new: boolean;
  workspace: boolean;
  model: boolean;
  mode?: boolean;
  thoughtLevel: boolean;
  sandboxMode?: boolean;
  approvalPolicy?: boolean;
  reply: boolean;
}

export type BotCommandPolicy = BotAllowedCommands;

export interface BotCurrentOptions {
  modelSelection?: ModelSelection;
  mode?: string;
  sandboxMode?: string;
  approvalPolicy?: string;
}

export type BotReplyMode = BotReplyGranularity;

export const BOT_HEARTBEAT_MIN_INTERVAL_MINUTES = 15;
export const BOT_HEARTBEAT_MAX_INTERVAL_MINUTES = 24 * 60;
export const BOT_HEARTBEAT_DEFAULT_INTERVAL_MINUTES = 60;

/** 群聊激活模式：disabled = 完全不响应群聊（默认，与旧行为一致）。 */
export type BotGroupActivation = "disabled" | "mention" | "always";

export interface BotGroupChatConfig {
  activation: BotGroupActivation;
  /** 旧配置遗留字段：群白名单已下线（群聊默认不限制任何群），解析时保留以兼容既有配置。 */
  allowedGroups?: string[];
}

/** 平台在群聊里的能力：决定「群聊方式」能提供哪些选项（与 MyAgents 的 groupActivation 对齐）。 */
export interface BotGroupChatCapabilities {
  /** 能可靠识别"机器人是否被 @" → 可提供「@提及」模式。 */
  mention: boolean;
  /**
   * 能收到未被 @ 的群消息 → 可提供「全部消息」模式。
   * 企微 AI Bot 平台只在被 @ 时下发群消息回调，收不到未 @ 的消息，因此为 false。
   */
  always: boolean;
}

/**
 * 群聊能力矩阵：
 * - telegram / feishu / lark：自带 mentions / entities，可精确判定被 @（mention 模式据此静默）；
 * - dingtalk：平台在回调里给 isInAtList，同样可判定；
 * - wecom：平台只在被 @ 时下发群回调，天然只有 mention 语义，不提供「全部消息」；
 * - weixin / webhook：拿不到 @ 状态，只能「全部消息」（或关闭）；retired provider 一律不支持。
 */
export function resolveBotGroupChatCapabilities(provider: BotProvider): BotGroupChatCapabilities {
  switch (provider) {
    case "telegram":
    case "feishu":
    case "lark":
    case "dingtalk":
      return { mention: true, always: true };
    case "wecom":
      return { mention: true, always: false };
    // 已下线通道（AstrBot 桥接）：不提供任何群聊模式。
    case "astrbot":
      return { mention: false, always: false };
    default:
      return { mention: false, always: true };
  }
}

/** 能可靠识别"机器人是否被 @"的平台；其余平台的群聊只提供 always/disabled。 */
export function botProviderSupportsGroupMention(provider: BotProvider): boolean {
  return resolveBotGroupChatCapabilities(provider).mention;
}

/**
 * 工作区身份键：workspaceIdentity 优先（同一路径的不同身份视为不同工作区）。
 * UI 与 service 共用同一个实现，保证"会话绑定资格"两端算出同一个 key。
 */
export function getBotWorkspaceKey(workspacePath: string, workspaceIdentity?: string): string {
  return workspaceIdentity?.trim() || workspacePath;
}

/**
 * 会话绑定资格：一个会话只能绑定"属于当前工作区"的机器人——
 * 工作区绑定表里列出的 bot，或 allowedWorkspaces 里显式包含本工作区的 bot。
 * 通配 "*"（含空数组）不算显式归属：否则任何全局 bot 都能绑进任何工作区，约束等于没有。
 */
/**
 * 心跳回合提示词的开头标记：只由 BotsService 的心跳生成。
 * 会话渲染层据此把整轮心跳（提示词 + HEARTBEAT_OK 答复）隐藏掉——
 * "心跳不写会话"，IM 侧仍按原规则（仅发现问题时汇报）投递。
 */
export const BOT_HEARTBEAT_PROMPT_MARKER_ZH = "[心跳] 主动检查";
export const BOT_HEARTBEAT_PROMPT_MARKER_EN = "[Heartbeat] Proactive check";

export function isBotHeartbeatPromptText(text: string): boolean {
  const trimmed = text.trimStart();
  return (
    trimmed.startsWith(BOT_HEARTBEAT_PROMPT_MARKER_ZH) ||
    trimmed.startsWith(BOT_HEARTBEAT_PROMPT_MARKER_EN)
  );
}

export function isBotEligibleForSessionBinding(params: {
  bot: { id: string; allowedWorkspaces: readonly string[] };
  workspaceKey: string;
  workspaceBoundBotIds: readonly string[];
}): boolean {
  if (params.workspaceBoundBotIds.includes(params.bot.id)) {
    return true;
  }
  const workspaceKey = params.workspaceKey.trim();
  if (!workspaceKey) {
    return false;
  }
  return params.bot.allowedWorkspaces.some((allowed) => {
    const trimmed = allowed.trim();
    return trimmed.length > 0 && trimmed !== ALL_BOT_WORKSPACES && trimmed === workspaceKey;
  });
}

/** 机器人心跳：定时唤醒 agent 检查工作区，有内容才汇报（HEARTBEAT_OK 抑制）。 */
export interface BotHeartbeatConfig {
  enabled: boolean;
  /** 触发间隔（分钟），15–1440。 */
  intervalMinutes: number;
  /** 旧配置遗留字段：活跃时段设置已下线，解析时保留以兼容既有配置。 */
  activeHours?: { start: string; end: string };
}

/** 私聊方式：bound_users = 仅绑定用户可驱动（默认）；all_users = 任何私聊用户都可驱动，无需绑定。 */
export type BotPrivateChatMode = "bound_users" | "all_users";

export interface BotConfig {
  id: string;
  name: string;
  provider: BotProvider;
  enabled: boolean;
  credentialRef?: string;
  webhookSecretRef?: string;
  webhookUrl?: string;
  webhookAuthHeaderName?: string;
  feishuAppId?: string;
  providerUserId?: string;
  /** 私聊方式；缺省按 bound_users（与历史行为一致）。 */
  privateChatMode?: BotPrivateChatMode;
  displayName?: string;
  /** 企业微信智能机器人 ID（botid）；secret 走 credentialRef 加密存储 */
  wecomBotId?: string;
  /** 钉钉应用 AppKey（clientId）；AppSecret 走 credentialRef 加密存储。robotCode 同 clientId。 */
  dingtalkClientId?: string;
  /** 钉钉 AI 卡片：开启后回复走卡片流式（需要卡片模板 ID），关闭则 Markdown 摘要。 */
  dingtalkUseAiCard?: boolean;
  /** 钉钉 AI 卡片模板 ID（useAiCard 为 true 时必填）。 */
  dingtalkCardTemplateId?: string;
  /**
   * 额外允许操控该机器人的平台用户 ID（绑定用户始终允许）。
   * 对齐 MyAgents 的 allowedUsers 白名单：绑定一个主用户后仍可授权同事。
   */
  allowedUsers?: string[];
  allowedWorkspaces: string[];
  /** 群聊激活模式；缺省 = disabled（群消息静默忽略）。 */
  groupChat?: BotGroupChatConfig;
  /** 心跳默认关闭；关闭时不产生任何后台唤醒。 */
  heartbeat?: BotHeartbeatConfig;
  allowedCommands: BotAllowedCommands;
  currentOptions: BotCurrentOptions;
  replyMode: BotReplyMode;
}

export interface BotPendingPermissionOption {
  requestId: string;
  optionId: string;
  command: "approve" | "deny";
  label: string;
  response: ModePermissionResponse;
  handledAt?: number;
}

export interface BotPendingElicitation {
  taskId: string;
  requestId: string;
  runId: string;
  origin?: ModeInteractionRequestOrigin;
  actorKey?: string;
  currentQuestionIndex: number;
  questions: ModeElicitationQuestion[];
  answers: Record<string, string[]>;
  renderContext?: {
    kind: "plan_approval";
    plan: string;
  };
  expandedCustomAnswerQuestionIndexes?: number[];
  handledAt?: number;
}

export interface BotStructuredElicitationResponse {
  requestId: string;
  action: "accept" | "decline" | "cancel";
  content?: Record<string, unknown>;
}

export interface BotOutboundElicitationRequest {
  requestId: string;
  taskId: string;
  runId: string;
  currentQuestionIndex: number;
  questions: ModeElicitationQuestion[];
  answers?: Record<string, string[]>;
  status?: "pending" | "completed" | "cancelled";
  expandedCustomAnswerQuestionIndexes?: number[];
  schema?: unknown;
}

export interface BotDraftOptions {
  provider: ModeProvider;
  modelSelection?: ModelSelection;
  mode?: string;
}

/** 任务运行中暂存的入站消息；任务终态后按序自动投递。 */
export interface BotQueuedMessage {
  text: string;
  receivedAt: number;
  /** 发送者私聊身份快照；重投递时用于重建 actor 与回执投递目标 */
  providerUserId?: string;
  displayName?: string;
  chatId?: string;
}

/** 出站投递失败后停放的回复（微信 context 过期等场景），随对话槽位持久化，新入站后按序补投。 */
export interface BotPendingDelivery {
  text: string;
  /** 与 BotOutboundMessage.providerUserId 同构：chatId ?? providerUserId。 */
  providerUserId: string;
  queuedAt: number;
  attempts: number;
}

export interface BotsConfigFile {
  version: 3;
  bots: BotConfig[];
}

/**
 * 对话（conversation）：私聊按用户、群聊按群。每个对话各自持有一个会话上下文
 * （工作区 + 桌面会话 + 草稿 + 挂起交互 + 排队消息），互不打断。
 */
export type BotConversationKind = "private" | "group";

export interface BotConversationRef {
  /** 稳定键：`private:<userId>` / `group:<chatId>`。 */
  key: string;
  kind: BotConversationKind;
  /** 私聊=对方 userId；群聊=群 chatId（缺 chatId 时退回发送者 id）。 */
  id: string;
}

export function makeBotConversationKey(kind: BotConversationKind, id: string): string {
  return `${kind}:${id}`;
}

/** 从会话键解析出对话身份；非法键返回 null。 */
export function parseBotConversationKey(
  key: string,
): { kind: BotConversationKind; id: string } | null {
  const separator = key.indexOf(":");
  if (separator <= 0 || separator === key.length - 1) {
    return null;
  }
  const kind = key.slice(0, separator);
  if (kind !== "private" && kind !== "group") {
    return null;
  }
  return { kind, id: key.slice(separator + 1) };
}

/**
 * 历史状态（v3 每 bot 单上下文）迁移时的占位键：没有可判定的对话身份时先用它承载，
 * 服务层拿到 bot 配置后会把绑定用户的私聊对话重映射到真正的键。
 */
export const BOT_LEGACY_CONVERSATION_KEY = "legacy";

/** 由入站消息推导对话身份：群聊按群、私聊按用户。 */
export function getBotActorConversation(
  actor: Pick<BotActor, "chatType" | "providerUserId" | "chatId">,
): BotConversationRef {
  const providerUserId = actor.providerUserId.trim();
  if (actor.chatType === "group") {
    const chatId = actor.chatId?.trim();
    const id = chatId && chatId.length > 0 ? chatId : providerUserId;
    return { key: makeBotConversationKey("group", id), kind: "group", id };
  }
  return {
    key: makeBotConversationKey("private", providerUserId),
    kind: "private",
    id: providerUserId,
  };
}

/** 对话级上下文：一个 IM 对话对应的桌面会话状态。 */
export interface BotConversationState {
  botId: string;
  /** 对话键（private:/group:），同一 bot 下唯一。 */
  conversationKey: string;
  conversationKind: BotConversationKind;
  /** 私聊=对方 userId；群聊=群 chatId。 */
  conversationId: string;
  /** 展示名：私聊为对方昵称（未知时省略），群聊为群 ID。 */
  conversationLabel?: string;
  workspacePath: string;
  workspaceIdentity?: string;
  workspaceId?: string;
  mode: BotContextMode;
  activeTaskId: string | null;
  draftOptions?: BotDraftOptions;
  pendingPermissionOptions?: BotPendingPermissionOption[];
  pendingElicitation?: BotPendingElicitation;
  queuedMessages?: BotQueuedMessage[];
  /** 最近一条入站消息携带的 provider context token；微信出站要求新鲜 token，过期会被服务端拒绝。 */
  lastContextToken?: string;
  /** 出站失败停放的回复；该对话下一条入站消息到达后按序补投。 */
  pendingDeliveryQueue?: BotPendingDelivery[];
  /** 当前失败 episode 已发过一次性提示的时间戳；任一投递成功后清除。 */
  deliveryNoticeAt?: number;
  updatedAt: number;
}

/** 兼容别名：业务代码里的"机器人上下文"现在就是某个对话的上下文。 */
export type BotContextState = BotConversationState;

/** 通道级状态：与具体对话无关，仍按 bot 唯一。 */
export interface BotChannelState {
  botId: string;
  /** 该 bot 的对话上下文表（key = conversationKey）。 */
  conversations: Record<string, BotConversationState>;
  /** 上次心跳触发时间；用于跨重启保持间隔语义（重启不立即补发）。 */
  lastHeartbeatAt?: number;
  telegramOffset?: number;
  /** 企微已处理的 msgid（含时间戳），跨重启去重：WS 重连时服务端可能重推未回执帧。 */
  wecomRecentMessageIds?: Array<{ id: string; at: number }>;
  /** 钉钉 Stream 已处理的 msgId（同企微机制）。 */
  dingtalkRecentMessageIds?: Array<{ id: string; at: number }>;
  weixinGetUpdatesBuf?: string;
  weixinActivatedAt?: number;
  updatedAt: number;
}

/** 兼容别名：v3 的 BotState 现在是"通道级状态 + 对话上下文"。 */
export type BotState = BotChannelState;

export const BOTS_STATE_FILE_VERSION = 4;

export interface BotsStateFile {
  version: typeof BOTS_STATE_FILE_VERSION;
  bots: Record<string, BotChannelState>;
}

export interface BotRuntimeInfo {
  /** 最近一次消息投递错误；独立于长连接状态，成功投递后清除。 */
  deliveryError?: string;
  botId: string;
  provider: BotProvider;
  status: "disabled" | "idle" | "polling" | "connected" | "error";
  messageId?: string;
  message?: string;
  lastUpdateAt?: number;
  offset?: number;
  /** 微信 Bot 是否已完成首次激活（通道级状态，UI 用它切换"等待扫码"文案）。 */
  weixinActivatedAt?: number;
}

export interface BotActor {
  provider: BotProvider;
  botId: string;
  providerUserId: string;
  displayName?: string;
  chatType: "private" | "group";
  chatId?: string;
  providerMessageId?: string;
  providerContextToken?: string;
  /** 群聊中是否 @ 了机器人；仅具备识别能力的平台会带此字段。 */
  isMention?: boolean;
}

export type BotInboundAttachmentKind = "image" | "audio" | "video" | "file";

export interface BotInboundAttachment {
  id: string;
  kind: BotInboundAttachmentKind;
  filename: string;
  mimeType: string;
  sizeBytes?: number;
  providerFileId?: string;
  downloadUrl?: string;
  dataBase64?: string;
  localPath?: string;
  providerMetadata?: Record<string, string>;
}

export type BotCommand =
  | { type: "bind"; code: string }
  | { type: "help" }
  | { type: "status" }
  | { type: "new" }
  | { type: "reconnect" }
  | { type: "workspace.list" }
  | { type: "workspace.set"; value: string }
  | { type: "model.list" }
  | { type: "model.provider.set"; value: string }
  | { type: "model.set"; value: string }
  | { type: "mode.list" }
  | { type: "mode.set"; value: string }
  | { type: "thoughtLevel.list" }
  | { type: "thoughtLevel.set"; value: string }
  | { type: "task.list" }
  | { type: "task.set"; value: string }
  | { type: "reply.list" }
  | { type: "reply.set"; value: string }
  | { type: "stop" }
  /** /send <path> [| caption]：把工作区/临时目录内的文件发到当前会话。 */
  | { type: "send"; value: string }
  | { type: "permission.respond"; value: string }
  | { type: "elicitation.respond"; value: string }
  | { type: "elicitation.submit" }
  | { type: "approve"; requestId: string; optionId: string }
  | { type: "deny"; requestId: string }
  | { type: "unknown"; name: string; raw: string }
  | { type: "selection.cancel" }
  | { type: "message"; text: string };

export interface SelectionPrompt {
  id: string;
  token?: string;
  title: string;
  currentId?: string;
  cancelLabel?: string;
  showCancel?: boolean;
  action:
    | "workspace.set"
    | "model.provider.set"
    | "model.set"
    | "mode.set"
    | "thoughtLevel.set"
    | "task.set"
    | "reply.set"
    | "permission.respond"
    | "elicitation.respond";
  options: Array<{
    id: string;
    label: string;
    description?: string;
  }>;
}

export interface BotInboundMessage {
  botId: string;
  actor: BotActor;
  text: string;
  attachments?: BotInboundAttachment[];
  elicitationResponse?: BotStructuredElicitationResponse;
  receivedAt?: number;
}

export interface BotOutboundMessage {
  botId: string;
  provider: BotProvider;
  providerUserId: string;
  text: string;
  locale?: Locale;
  selection?: SelectionPrompt;
  elicitation?: BotOutboundElicitationRequest;
  providerContextToken?: string;
}

export interface BotTaskSummary {
  taskId: string;
  title: string;
  status: ModeTaskRuntimeStatus | "persisted-completed" | "persisted-error" | "unknown";
  workspacePath: string;
  workspaceIdentity?: string;
  provider?: ModeProvider;
  model?: string;
}

export const BOT_TASK_BROADCAST_CHANNEL = "bots:task";
export const BOT_TASK_STREAM_BROADCAST_CHANNEL = "bots:task-stream";

export type BotTaskBroadcastEvent =
  | "created"
  | "prompt_sent"
  | "resumed"
  | "streaming"
  | "permission_request"
  | "permission_resolved"
  | "elicitation_request"
  | "elicitation_resolved"
  | "updated"
  | "completed"
  | "error"
  /** bot 侧（/task.set、/new、新建任务）切换了目标会话，UI 应跟随跳转 */
  | "active_task_changed";

export type BotTaskBroadcastSource = "bot" | "ui";

export interface BotTaskBroadcastPayload {
  workspacePath: string;
  workspaceIdentity?: string;
  taskId: string;
  event: BotTaskBroadcastEvent;
  /** 事件发起方；UI 侧用它区分“bot 发起的跳转”与“UI 自己触发的回声”，防止同步循环 */
  source?: BotTaskBroadcastSource;
  updatedAt: number;
  task?: ModeTaskMeta;
  provider?: ModeProvider;
  configOptions?: ModeConfigOption[];
  prompt?: {
    content: string;
    attachments?: ModePromptAttachment[];
    messageId: string;
    sentAt: number;
  };
  permissionRequest?: ModePermissionRequest;
  elicitationRequest?: ModeElicitationRequest;
  requestId?: string;
  error?: string;
}

export interface BotTaskStreamBroadcastPayload {
  workspacePath: string;
  workspaceIdentity?: string;
  taskId: string;
  event: ModeStreamEvent;
  updatedAt: number;
}

export interface BotServiceStatus {
  botsCount: number;
  enabledBotsCount: number;
  contextsCount: number;
  botRuntime: BotRuntimeInfo[];
}

export interface BotProviderCallbackResult {
  ok: boolean;
  replies: BotOutboundMessage[];
  responseBody?: unknown;
  status?: number;
}

export const botAllowedCommandsSchema = z
  .object({
    status: z.boolean(),
    new: z.boolean(),
    workspace: z.boolean(),
    model: z.boolean(),
    mode: z.boolean().optional(),
    thoughtLevel: z.boolean(),
    sandboxMode: z.boolean().optional(),
    approvalPolicy: z.boolean().optional(),
    // 兼容旧 bot-config.json；/cli 命令已移除，新配置不会再写入这个字段。
    cli: z.boolean().optional(),
    reply: z.boolean(),
  })
  .strict();

export const botCommandPolicySchema = botAllowedCommandsSchema;

export const botCurrentOptionsSchema = z
  .object({
    modelSelection: modelSelectionSchema.optional(),
    mode: z.string().min(1).optional(),
    sandboxMode: z.string().min(1).optional(),
    approvalPolicy: z.string().min(1).optional(),
    // 兼容旧 bot-config.json；CLI provider 现在统一由 Mode Protocol 侧配置决定。
    cli: z.literal(MODE_AGENT_PROVIDER).optional(),
  })
  .strict();

export const botDraftOptionsSchema = z
  .object({
    provider: z.literal(MODE_AGENT_PROVIDER),
    modelSelection: modelSelectionSchema.optional(),
    mode: z.string().min(1).optional(),
  })
  .strict();

const botElicitationOptionSchema = z
  .object({
    value: z.string(),
    label: z.string(),
    description: z.string().optional(),
  })
  .strict();

const botElicitationQuestionSchema = z
  .object({
    question: z.string(),
    header: z.string(),
    options: z.array(botElicitationOptionSchema),
    multiSelect: z.boolean().optional(),
  })
  .strict();

const botPendingElicitationSchema = z
  .object({
    taskId: z.string().min(1),
    requestId: z.string().min(1),
    runId: z.string().min(1),
    origin: modeInteractionRequestOriginSchema.optional(),
    actorKey: z.string().min(1).optional(),
    currentQuestionIndex: z.number().int().min(0),
    questions: z.array(botElicitationQuestionSchema),
    answers: z.record(z.string(), z.array(z.string())),
    renderContext: z
      .object({
        kind: z.literal("plan_approval"),
        plan: z.string().min(1),
      })
      .strict()
      .optional(),
    expandedCustomAnswerQuestionIndexes: z.array(z.number().int().min(0)).optional(),
    handledAt: z.number().optional(),
  })
  .strict();

export const botConfigSchema = z
  .object({
    id: z.string().min(1),
    name: z.string(),
    provider: z.enum(botProviders),
    enabled: z.boolean(),
    credentialRef: z.string().min(1).optional(),
    webhookSecretRef: z.string().min(1).optional(),
    webhookUrl: z.string().url().optional(),
    webhookAuthHeaderName: z.string().min(1).optional(),
    feishuAppId: z.string().min(1).optional(),
    providerUserId: z.string().min(1).optional(),
    privateChatMode: z.enum(["bound_users", "all_users"]).optional(),
    displayName: z.string().optional(),
    wecomBotId: z.string().min(1).optional(),
    dingtalkClientId: z.string().min(1).optional(),
    dingtalkUseAiCard: z.boolean().optional(),
    dingtalkCardTemplateId: z.string().min(1).optional(),
    allowedUsers: z.array(z.string().min(1)).optional(),
    allowedWorkspaces: z.array(z.string().min(1)),
    groupChat: z
      .object({
        activation: z.enum(["disabled", "mention", "always"]),
        // 旧配置遗留字段：群白名单已下线，保留以便老配置继续解析（不再参与任何判定）。
        allowedGroups: z.array(z.string().min(1)).optional(),
      })
      .strict()
      .optional(),
    // 警告：BotConfig 的每个新增字段都必须同时登记到这里——
    // botConfigSchema 是 .strict()，只加 TS 类型会让保存直接抛 ZodError（"Unrecognized key"）。
    heartbeat: z
      .object({
        enabled: z.boolean(),
        intervalMinutes: z.number(),
        activeHours: z
          .object({
            start: z.string(),
            end: z.string(),
          })
          .strict()
          .optional(),
      })
      .strict()
      .optional(),
    allowedCommands: botAllowedCommandsSchema,
    currentOptions: botCurrentOptionsSchema,
    replyMode: z.enum([
      "assistant_changes",
      "assistant_toolcalls_changes",
      "summary_changes",
      "streaming_card",
    ]),
  })
  .strict();

/** 已下线通道：这些 provider 的机器人记录在配置解析阶段直接丢弃。 */
export const RETIRED_BOT_PROVIDERS: readonly string[] = ["astrbot"];

function stripRetiredBotProviders(value: unknown): unknown {
  if (!value || typeof value !== "object") {
    return value;
  }
  const candidate = value as { bots?: unknown };
  if (!Array.isArray(candidate.bots)) {
    return value;
  }
  return {
    ...candidate,
    bots: candidate.bots.filter((bot) => {
      const provider = (bot as { provider?: unknown } | null)?.provider;
      return typeof provider !== "string" || !RETIRED_BOT_PROVIDERS.includes(provider);
    }),
  };
}

export const botsConfigFileSchema = z.preprocess(
  stripRetiredBotProviders,
  z
    .object({
      version: z.literal(3),
      bots: z.array(botConfigSchema),
    })
    .strict(),
);

/** 工作区 → bot 绑定表（bot-bindings.v3.json）：与设置文件分离，避免被其它设置写入方覆盖。 */
export const BOT_BINDINGS_FILE_VERSION = 2;

/** 工作区 → bot[] 绑定表（一个工作区可绑定多个机器人）。 */
export type BotWorkspaceBindingsMap = Record<string, string[]>;

/**
 * 归一化绑定表：兼容 v1 的「一个工作区一个 bot」（字符串）与 v2 的字符串数组，
 * 统一产出 `Record<workspaceKey, botId[]>`（去空白、去重、丢掉空值/空列表）。
 */
export function normalizeBotWorkspaceBindings(
  bindings: Record<string, string | readonly string[]> | undefined,
): BotWorkspaceBindingsMap {
  const normalized: BotWorkspaceBindingsMap = {};
  for (const [workspaceKey, value] of Object.entries(bindings ?? {})) {
    const key = workspaceKey.trim();
    if (!key) {
      continue;
    }
    const botIds = (Array.isArray(value) ? value : [value])
      .map((botId) => botId.trim())
      .filter(Boolean);
    if (botIds.length === 0) {
      continue;
    }
    normalized[key] = [...new Set(botIds)];
  }
  return normalized;
}

export const botBindingsFileSchema = z
  .object({
    version: z.union([z.literal(1), z.literal(2)]),
    // v1 读侧兼容单 bot 字符串；写侧一律 v2 数组（见 BOT_BINDINGS_FILE_VERSION）。
    bindings: z.record(z.string(), z.union([z.string(), z.array(z.string())])),
  })
  .strict();

export type BotBindingsFile = z.infer<typeof botBindingsFileSchema>;

/** 对话上下文（v4）的 schema：对话身份 + 会话状态。 */
const botConversationStateSchema = z.object({
  botId: z.string().min(1),
  conversationKey: z.string().min(1),
  conversationKind: z.enum(["private", "group"]),
  conversationId: z.string().min(1),
  conversationLabel: z.string().min(1).optional(),
  workspacePath: z.string().min(1),
  workspaceIdentity: z.string().min(1).optional(),
  workspaceId: z.string().min(1).optional(),
  mode: z.enum(["draft", "task"]),
  activeTaskId: z.string().min(1).nullable(),
  draftOptions: botDraftOptionsSchema.optional(),
  pendingPermissionOptions: z
    .array(
      z.object({
        requestId: z.string().min(1),
        optionId: z.string().min(1),
        command: z.enum(["approve", "deny"]),
        label: z.string().min(1),
        response: modePermissionResponseSchema,
        handledAt: z.number().optional(),
      }),
    )
    .optional(),
  pendingElicitation: botPendingElicitationSchema.optional(),
  queuedMessages: z
    .array(
      z
        .object({
          text: z.string(),
          receivedAt: z.number(),
          providerUserId: z.string().min(1).optional(),
          displayName: z.string().optional(),
          chatId: z.string().optional(),
        })
        .strict(),
    )
    .optional(),
  lastContextToken: z.string().optional(),
  pendingDeliveryQueue: z
    .array(
      z
        .object({
          text: z.string(),
          providerUserId: z.string().min(1),
          queuedAt: z.number(),
          attempts: z.number(),
        })
        .strict(),
    )
    .optional(),
  deliveryNoticeAt: z.number().optional(),
  updatedAt: z.number(),
});

/** 通道级状态（v4）：与对话无关的游标/去重表 + 该 bot 的对话上下文表。 */
const botChannelStateSchema = z.object({
  botId: z.string().min(1),
  conversations: z.record(z.string().min(1), botConversationStateSchema),
  lastHeartbeatAt: z.number().optional(),
  telegramOffset: z.number().optional(),
  wecomRecentMessageIds: z
    .array(
      z
        .object({
          id: z.string().min(1),
          at: z.number(),
        })
        .strict(),
    )
    .optional(),
  dingtalkRecentMessageIds: z
    .array(
      z
        .object({
          id: z.string().min(1),
          at: z.number(),
        })
        .strict(),
    )
    .optional(),
  weixinGetUpdatesBuf: z.string().optional(),
  weixinActivatedAt: z.number().optional(),
  updatedAt: z.number(),
});

interface LegacyBotState {
  botId?: unknown;
  workspacePath?: unknown;
  workspaceIdentity?: unknown;
  workspaceId?: unknown;
  mode?: unknown;
  activeTaskId?: unknown;
  draftOptions?: unknown;
  pendingPermissionOptions?: unknown;
  pendingElicitation?: unknown;
  queuedMessages?: unknown;
  lastPrivateUserId?: unknown;
  lastHeartbeatAt?: unknown;
  telegramOffset?: unknown;
  wecomRecentMessageIds?: unknown;
  dingtalkRecentMessageIds?: unknown;
  weixinGetUpdatesBuf?: unknown;
  weixinActivatedAt?: unknown;
  updatedAt?: unknown;
}

function readLegacyString(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() !== "" ? value : undefined;
}

/**
 * v3（每 bot 单上下文）→ v4（按对话）：把旧上下文挂到能判定的对话上。
 * 判定顺序：最近私聊用户 → 队列里出现过的群 → 队列里出现过的私聊用户 → legacy 占位键。
 * legacy 键无法匹配任何真实入站消息，服务层拿到 bot 配置后会把它重映射到绑定用户的私聊对话；
 * 实在无法归属时该对话的历史会话绑定会失效（工作区绑定与授权不受影响）。
 */
function importLegacyConversationContext(entry: LegacyBotState): BotConversationState | null {
  const botId = readLegacyString(entry.botId);
  const workspacePath = readLegacyString(entry.workspacePath);
  if (!botId || !workspacePath) {
    return null;
  }
  const queued = Array.isArray(entry.queuedMessages) ? entry.queuedMessages : [];
  const queuedRecord = queued.filter(
    (item): item is Record<string, unknown> => typeof item === "object" && item !== null,
  );
  const lastPrivateUserId = readLegacyString(entry.lastPrivateUserId);
  const queuedChatId = queuedRecord
    .map((item) => readLegacyString(item.chatId))
    .find((value) => value !== undefined);
  const queuedPrivateUserId = queuedRecord
    .map((item) => readLegacyString(item.providerUserId))
    .find((value) => value !== undefined);
  let key = BOT_LEGACY_CONVERSATION_KEY;
  if (lastPrivateUserId) {
    key = makeBotConversationKey("private", lastPrivateUserId);
  } else if (queuedChatId) {
    key = makeBotConversationKey("group", queuedChatId);
  } else if (queuedPrivateUserId) {
    key = makeBotConversationKey("private", queuedPrivateUserId);
  }
  const identity =
    key === BOT_LEGACY_CONVERSATION_KEY
      ? { kind: "private" as const, id: BOT_LEGACY_CONVERSATION_KEY }
      : (parseBotConversationKey(key) ?? { kind: "private" as const, id: BOT_LEGACY_CONVERSATION_KEY });
  return {
    botId,
    conversationKey: key,
    conversationKind: identity.kind,
    conversationId: identity.id,
    workspacePath,
    ...(readLegacyString(entry.workspaceIdentity)
      ? { workspaceIdentity: readLegacyString(entry.workspaceIdentity) }
      : {}),
    ...(readLegacyString(entry.workspaceId) ? { workspaceId: readLegacyString(entry.workspaceId) } : {}),
    mode: entry.mode === "task" ? "task" : "draft",
    activeTaskId: readLegacyString(entry.activeTaskId) ?? null,
    ...(entry.draftOptions !== undefined
      ? { draftOptions: entry.draftOptions as BotDraftOptions }
      : {}),
    ...(entry.pendingPermissionOptions !== undefined
      ? { pendingPermissionOptions: entry.pendingPermissionOptions as BotPendingPermissionOption[] }
      : {}),
    ...(entry.pendingElicitation !== undefined
      ? { pendingElicitation: entry.pendingElicitation as BotPendingElicitation }
      : {}),
    ...(entry.queuedMessages !== undefined
      ? { queuedMessages: entry.queuedMessages as BotQueuedMessage[] }
      : {}),
    updatedAt: typeof entry.updatedAt === "number" ? entry.updatedAt : Date.now(),
  };
}

/** v3 状态文件 → v4：通道级字段保留在 bot 记录上，上下文按对话挂载。 */
export function migrateBotsStateFileV3(input: unknown): unknown {
  if (typeof input !== "object" || input === null) {
    return input;
  }
  const record = input as Record<string, unknown>;
  if (record.version !== 3 || typeof record.bots !== "object" || record.bots === null) {
    return input;
  }
  const bots: Record<string, unknown> = {};
  for (const [botId, rawEntry] of Object.entries(record.bots as Record<string, unknown>)) {
    if (typeof rawEntry !== "object" || rawEntry === null) {
      continue;
    }
    const entry = rawEntry as LegacyBotState;
    const conversations: Record<string, BotConversationState> = {};
    const context = importLegacyConversationContext({ ...entry, botId: entry.botId ?? botId });
    if (context) {
      conversations[context.conversationKey] = context;
    }
    bots[botId] = {
      botId,
      conversations,
      ...(entry.lastHeartbeatAt !== undefined ? { lastHeartbeatAt: entry.lastHeartbeatAt } : {}),
      ...(entry.telegramOffset !== undefined ? { telegramOffset: entry.telegramOffset } : {}),
      ...(entry.wecomRecentMessageIds !== undefined
        ? { wecomRecentMessageIds: entry.wecomRecentMessageIds }
        : {}),
      ...(entry.dingtalkRecentMessageIds !== undefined
        ? { dingtalkRecentMessageIds: entry.dingtalkRecentMessageIds }
        : {}),
      ...(entry.weixinGetUpdatesBuf !== undefined
        ? { weixinGetUpdatesBuf: entry.weixinGetUpdatesBuf }
        : {}),
      ...(entry.weixinActivatedAt !== undefined ? { weixinActivatedAt: entry.weixinActivatedAt } : {}),
      updatedAt: typeof entry.updatedAt === "number" ? entry.updatedAt : Date.now(),
    };
  }
  return { version: BOTS_STATE_FILE_VERSION, bots };
}

export const botsStateFileSchema = z.preprocess(
  migrateBotsStateFileV3,
  z
    .object({
      version: z.literal(BOTS_STATE_FILE_VERSION),
      bots: z.record(z.string(), botChannelStateSchema),
    })
    .strict(),
);

export const DEFAULT_BOT_COMMANDS: BotAllowedCommands = {
  status: true,
  new: true,
  workspace: true,
  model: true,
  mode: true,
  thoughtLevel: true,
  reply: true,
};

export const DEFAULT_BOT_REPLY_GRANULARITY: BotReplyGranularity = "assistant_changes";

export function getSupportedBotReplyGranularities(
  provider: BotProvider,
): readonly BotReplyGranularity[] {
  if (isFeishuBotProvider(provider)) {
    return ["streaming_card"] as const;
  }
  if (provider === "telegram" || provider === "wecom" || provider === "dingtalk") {
    // Telegram 复用 streaming_card 管线：sendMessage 拿 message_id + editMessageText 节流编辑。
    // 企业微信智能机器人经 WebSocket replyStream 原生支持流式回复（同一 stream 增量刷新）。
    // 钉钉走 AI 卡片流式（card/streaming），未配置模板时由服务层回退为 Markdown 摘要。
    return BOT_REPLY_GRANULARITIES;
  }
  return BOT_REPLY_GRANULARITIES.filter(
    // Bugfix: streaming card 依赖 Feishu/Lark Card JSON 2.0，其他 channel 无法渲染或更新该消息形态。
    (granularity) => granularity !== "streaming_card",
  );
}

export function normalizeBotReplyGranularity(
  provider: BotProvider,
  replyMode: BotReplyGranularity | undefined,
): BotReplyGranularity {
  const supported = getSupportedBotReplyGranularities(provider);
  const candidate = replyMode ?? DEFAULT_BOT_REPLY_GRANULARITY;
  return supported.includes(candidate) ? candidate : supported[0]!;
}

export const BOT_MODE_PROVIDER_OPTIONS: Array<{
  id: ModeProvider;
  label: string;
}> = [{ id: MODE_AGENT_PROVIDER, label: MODE_AGENT_PROVIDER_LABEL }];
