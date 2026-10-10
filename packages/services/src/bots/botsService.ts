/* eslint-disable max-lines -- Bots 服务仍复用原 RPC 文件名，先把鉴权、命令路由、Mode Agent 桥接收口集中在同一服务内。 */
import { Buffer } from "node:buffer";
import { createHash, randomBytes } from "node:crypto";
import { mkdir, readFile, realpath, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, dirname, isAbsolute, join, resolve as resolvePath } from "node:path";
import type { IDisposable } from "@mode/rpc";
import { completeNewModelSelection } from "@mode/provider";
import {
  ALL_BOT_WORKSPACES,
  generateTraceId,
  normalizeAgentProviderToModeAgent,
  MODE_AGENT_PROVIDER,
  BOT_TASK_BROADCAST_CHANNEL,
  BOT_TASK_STREAM_BROADCAST_CHANNEL,
  appendAssistantMessagePart,
  buildModeAssistantPresentation,
  decodeCustomModelValue,
  encodeCustomModelValue,
  getPermissionRequestPreview,
  getSupportedBotReplyGranularities,
  normalizeBotReplyGranularity,
  type ModeConfigOption,
  type ModeElicitationRequest,
  type ModeElicitationQuestion,
  type ModePermissionOption,
  type ModePermissionRequest,
  type ModePromptAttachment,
  type ModeTaskMode,
  type ModeAssistantMessagePart,
  type ModeAutomationBotDeliveryTarget,
  type ModeProvider,
  type ModeStreamEvent,
  type TaskStreamMirrorableEvent,
  type ModeTaskMeta,
  type BotActor,
  type BotTaskBroadcastPayload,
  type BotTaskStreamBroadcastPayload,
  type BotConfig,
  type BotChannelState,
  type BotContextState,
  type BotConversationKind,
  type BotConversationState,
  type BotDraftOptions,
  type BotCommand,
  type BotInboundAttachment,
  type BotInboundMessage,
  type BotOutboundMessage,
  type BotPendingElicitation,
  type BotQueuedMessage,
  type BotStructuredElicitationResponse,
  isFeishuBotProvider,
  isBotEligibleForSessionBinding,
  normalizeBotWorkspaceBindings,
  BOT_BINDINGS_FILE_VERSION,
  type BotProvider,
  type BotProviderCallbackResult,
  type BotReplyGranularity,
  type BotRuntimeInfo,
  type BotWorkspaceRef,
  type BotsStateFile,
  type ModelSelection,
  type BotsConfigFile,
  BOT_HEARTBEAT_PROMPT_MARKER_EN,
  BOT_HEARTBEAT_PROMPT_MARKER_ZH,
  BOT_LEGACY_CONVERSATION_KEY,
  getBotActorConversation,
  makeBotConversationKey,
  parseBotConversationKey,
  type Locale,
  type SelectionPrompt,
} from "@mode/shared";
import type { IModeTaskService } from "../session/modeTaskService.js";
import { resolveProviderModeIdFromConfigOptions } from "#src/session/sessionModeOptions.js";
import { deriveSessionTitle as deriveTaskTitle } from "#src/session/sessionTitle.js";
import type { IBroadcastService } from "../broadcast/broadcast.js";
import type { ICredentialService } from "../credential/credential.js";
import { getAppConfigDir } from "../paths.js";
import type { ISettingService } from "../setting/setting.js";
import type {
  IModelSelectionService,
  ModelSelectionView,
} from "../model-provider/providerFacadeServices.js";
import type { ModeAgentAppRuntimePreferences } from "../mode-agent/modeAgent.js";
import { createServiceLogger } from "#src/logger/serviceLogger.js";
import type {
  BotBindCodeResult,
  BotAutomationRunWatchParams,
  BotCreateBindCodeParams,
  BotListWorkspaceRefsParams,
  BotSaveBotParams,
  BotTestResult,
  BotSendMediaParams,
  BotUiFocusParams,
  BotUserConfigOptionsParams,
  BotWorkspaceBindingInfo,
  BotWorkspaceBindingParams,
  IBotsService,
} from "./bots.js";
import {
  beginFeishuAppRegistration,
  pollFeishuAppRegistration,
} from "./providers/feishuAppRegistration.js";
import {
  BOT_BIND_CODE_TTL_MS,
  buildBotCredentialKey,
  buildBotWebhookSecretKey,
  getDefaultBotReplyGranularity,
  normalizeBotCommandPolicy,
  normalizeBotCurrentOptions,
} from "./config.js";
import { BOT_MENU_COMMAND_ORDER } from "./commandOrder.js";
import { parseBotCommand } from "./commandParser.js";
import { BotsRepo } from "./repo.js";
import type {
  BotProviderAdapter,
  BotStreamingReplyCardBlock,
  BotStreamingReplyCardHandle,
  BotTransientInteractionCardHandle,
  BotTypingTarget,
} from "./providers/types.js";
import { createTelegramBotProvider } from "./providers/telegramProvider.js";
import { createWebhookBotProvider } from "./providers/webhookProvider.js";
import { createWeixinBotProvider } from "./providers/weixinProvider.js";
import { createWeComBotProvider } from "./providers/wecomProvider.js";
import {
  createDingtalkBotProvider,
  isDingtalkAiCardEnabled,
} from "./providers/dingtalkProvider.js";
import {
  beginWeComRegistration as beginWeComQrRegistration,
  pollWeComRegistration as pollWeComQrRegistration,
} from "./providers/wecomRegistration.js";
import { createWeComChannelRuntime } from "./wecomChannelRuntime.js";
import { createDingtalkChannelRuntime } from "./dingtalkChannelRuntime.js";
import { createWeComConnectionRegistry } from "./wecomConnection.js";
import {
  beginWeixinRegistration as beginWeixinQrRegistration,
  pollWeixinRegistration as pollWeixinQrRegistration,
} from "./providers/weixinRegistration.js";
import { createFeishuBotProvider } from "./providers/feishuProvider.js";
import { formatBotMessage, type BotMessageId } from "./messages.js";
import { hasSentenceBoundary } from "./botText.js";
import {
  createGroupHistoryBuffer,
  formatGroupHistoryContext,
} from "./groupHistory.js";
import {
  buildGroupTurnPrompt,
  formatBotProviderLabel,
  GROUP_CHAT_TOOL_DENYLIST,
  isGroupSilenceReply,
  type GroupTurnPromptInput,
} from "./groupPrompt.js";
import {
  isHeartbeatDue,
  isHeartbeatOkOnly,
  normalizeBotHeartbeat,
} from "./botHeartbeat.js";
import {
  BOT_MEDIA_FILE_LIMIT_BYTES,
  BOT_MEDIA_IMAGE_LIMIT_BYTES,
  isPathInside,
  resolveOutboundMediaKind,
} from "./botMedia.js";
import {
  enqueueQueuedMessage,
  dequeueQueuedMessage,
  type EnqueueQueuedMessageResult,
} from "./messageQueue.js";
import {
  BOT_SEND_MIN_INTERVAL_MS,
  BotSendError,
  bumpFirstPendingDeliveryAttempt,
  classifySendError,
  createBotSendQueue,
  enqueuePendingDelivery,
  planDeliveryRecovery,
  removeFirstPendingDelivery,
  type BotSendFailureCode,
} from "./outboundDelivery.js";
import {
  getWorkspaceBoundBots,
  removeBotFromOtherWorkspaces,
  removeWorkspaceBinding,
  resolveBoundWorkspaceRefs,
  setWorkspaceBinding,
  type BotWorkspaceBindings,
} from "./botsBinding.js";
import {
  extractBotAssistantResponseMessages,
  formatBotAssistantReplyBlocks,
  formatBotToolCallSummaryLine,
  formatBotPermissionRequestSummary,
  formatBotToolCallReply,
  isBotToolCallReplyTerminal,
  updateBotReplyToolCalls,
  type BotAssistantReplyBlock,
  type BotReplyToolCallState,
} from "./replyFormatter.js";
import {
  findBoundUser,
  findAuthorizedBot,
  findCallbackBot,
  findBot,
  getContextKey,
  isUserCommandAllowed,
  normalizeBotConfig,
  normalizeConfigBots,
} from "./botConfigHelpers.js";
import {
  firstAllowedWorkspace,
  createWorkspaceRef,
  filterAllowedWorkspaces,
  getWorkspaceLabel,
  getWorkspaceKey,
  isWorkspaceAllowed,
  normalizeAllowedWorkspaces,
  normalizeConfiguredAllowedWorkspaces,
  resolveLegacyBindingWorkspaceKey,
  resolveWorkspaceByValue,
} from "./workspaceHelpers.js";
import { getNativeModelProviderId } from "./modelSelectionHelpers.js";
import {
  formatStatusStreamToolProgress,
  formatStatusTaskLine,
  formatTaskRunningDuration,
  normalizeStatusProgressText,
  readLatestAssistantTurnChangeSummary,
  readLatestTaskProgress,
  readTaskWorkedDurationMs,
  taskStatus,
  truncateLiveStatusProgressText,
} from "./statusFormatting.js";
import { createTelegramChannelRuntime } from "./telegramChannelRuntime.js";
import { createWeixinChannelRuntime } from "./weixinChannelRuntime.js";
import { createFeishuChannelRuntime } from "./feishuChannelRuntime.js";
import { registerMemoryDiagnosticsProvider } from "#src/memoryDiagnostics.js";

const botsLogger = createServiceLogger("bots");

function formatBotModelSelectionValue(selection: ModelSelection | undefined): string | undefined {
  if (!selection) return undefined;
  return selection.providerId === MODE_AGENT_PROVIDER
    ? selection.modelId
    : encodeCustomModelValue(selection.providerId, selection.modelId);
}

function parseBotModelOptionValue(value: string): ModelSelection | undefined {
  const decoded = decodeCustomModelValue(value);
  if (decoded?.providerId && decoded.modelName) {
    return { providerId: decoded.providerId, modelId: decoded.modelName };
  }
  const separatorIndex = value.indexOf("/");
  if (separatorIndex > 0 && separatorIndex < value.length - 1) {
    return {
      providerId: value.slice(0, separatorIndex),
      modelId: value.slice(separatorIndex + 1),
    };
  }
  return value.trim() ? { providerId: MODE_AGENT_PROVIDER, modelId: value.trim() } : undefined;
}

const BOT_REPLY_GRANULARITY_OPTIONS = [
  {
    id: "assistant_changes",
    label: { "zh-CN": "标准回复", "en-US": "Standard reply" },
    aliases: ["assistant", "assistant_changes", "normal", "default", "standard", "标准回复"],
  },
  {
    id: "assistant_toolcalls_changes",
    label: { "zh-CN": "完整回复", "en-US": "Full reply" },
    aliases: ["full", "tool", "toolcalls", "assistant_toolcalls_changes", "完整回复"],
  },
  {
    id: "summary_changes",
    label: { "zh-CN": "摘要回复", "en-US": "Summary reply" },
    aliases: ["summary", "summary_changes", "latest", "摘要回复"],
  },
  {
    id: "streaming_card",
    label: { "zh-CN": "流式卡片", "en-US": "Streaming card" },
    aliases: ["stream", "streaming", "streaming_card", "流式", "流式卡片"],
  },
] as const satisfies ReadonlyArray<{
  id: BotReplyGranularity;
  label: Record<"zh-CN" | "en-US", string>;
  aliases: readonly string[];
}>;

const BOT_EXCLUSIVE_CREDENTIAL_PROVIDERS = new Set<BotProvider>([
  "telegram",
  "feishu",
  "lark",
  // 钉钉：一个 AppKey/AppSecret 只应有一个启用的机器人消费 Stream 长连接。
  "dingtalk",
]);
const FEISHU_STREAMING_CARD_MIN_UPDATE_INTERVAL_MS = 1_000;
const FEISHU_STREAMING_CARD_REQUEST_TIMEOUT_MS = 15_000;
const FEISHU_STREAMING_CARD_FAILURE_BACKOFF_BASE_MS = 1_000;
const FEISHU_STREAMING_CARD_FAILURE_CIRCUIT_THRESHOLD = 3;
const BOT_ELICITATION_PROGRESS_BROADCAST_TIMEOUT_MS = 1_000;
const BOT_PROVIDER_CALLBACK_ACK_TIMEOUT_MS = 3_000;



/** 各 provider 的流式回复最小编辑间隔；Telegram 编辑有速率压力，与飞书保持同一档。 */
const STREAMING_CARD_MIN_UPDATE_INTERVAL_MS_BY_PROVIDER: Partial<Record<BotProvider, number>> = {
  telegram: 1_000,
  feishu: FEISHU_STREAMING_CARD_MIN_UPDATE_INTERVAL_MS,
  lark: FEISHU_STREAMING_CARD_MIN_UPDATE_INTERVAL_MS,
};

type StreamingCardTimelineBlock =
  | {
      type: "message";
      text: string;
    }
  | {
      type: "tools";
      toolIds: string[];
    };

const helpMessageByCommand = {
  help: "helpHelp",
  bind: "helpBind",
  status: "helpStatus",
  new: "helpNew",
  workspace: "helpWorkspace",
  model: "helpModel",
  mode: "helpMode",
  thoughtLevel: "helpThoughtLevel",
  reply: "helpReply",
} as const satisfies Record<(typeof BOT_MENU_COMMAND_ORDER)[number], BotMessageId>;

function validateBotConfig(config: BotsConfigFile, candidate: BotConfig): void {
  if (!candidate.id.trim()) {
    throw new Error("Bot id is required.");
  }
  if (candidate.enabled && candidate.providerUserId?.trim()) {
    const duplicateBinding = config.bots.find(
      (bot) =>
        bot.id !== candidate.id &&
        bot.enabled &&
        bot.provider === candidate.provider &&
        bot.providerUserId === candidate.providerUserId,
    );
    if (duplicateBinding) {
      throw new Error("An enabled bot with this provider user already exists.");
    }
  }
  if (
    candidate.enabled &&
    candidate.credentialRef?.trim() &&
    BOT_EXCLUSIVE_CREDENTIAL_PROVIDERS.has(candidate.provider)
  ) {
    const duplicateCredential = config.bots.find(
      (bot) =>
        bot.id !== candidate.id &&
        bot.enabled &&
        bot.provider === candidate.provider &&
        bot.credentialRef === candidate.credentialRef,
    );
    if (duplicateCredential) {
      throw new Error("Enabled polling bots cannot share the same credential.");
    }
  }
}

interface BotsServiceDeps {
  credentialService: ICredentialService;
  modeTaskService: IModeTaskService;
  broadcastService?: IBroadcastService;
  settingService?: ISettingService;
  modelSelectionService: Pick<IModelSelectionService, "getView">;
  remoteWorkspaceService?: BotRemoteWorkspaceService;

  // 修复原因：desktop-attached 远端启动阶段不应抢跑 bot 轮询、runtime lock 和模型候选缓存；
  // 这些后台任务属于本地桌面 host，不属于 SSH/Docker 远端首屏连接路径。
  runStartupBackgroundTasks?: boolean;
}

interface BotRemoteWorkspaceTarget {
  workspacePath: string;
  workspaceIdentity: string;
}

interface BotRemoteWorkspaceReconnectResult {
  ok: boolean;
  message?: string;
}

interface BotRemoteWorkspaceService {
  isConnected(target: BotRemoteWorkspaceTarget): Promise<boolean>;
  ensureConnected(target: BotRemoteWorkspaceTarget): Promise<BotRemoteWorkspaceReconnectResult>;
  getModeTaskService?(target: BotRemoteWorkspaceTarget): Promise<IModeTaskService | null>;
  getModelSelectionService?(
    target: BotRemoteWorkspaceTarget,
  ): Promise<Pick<IModelSelectionService, "getView"> | null>;
  syncAppRuntimePreferences?(preferences: ModeAgentAppRuntimePreferences): Promise<void>;
}

interface PreparedBotMessageContent {
  content: string;
  modeAttachments: ModePromptAttachment[];
}

type BotAuthorizedCommand =
  | "help"
  | "status"
  | "new"
  | "reconnect"
  | "workspace"
  | "model"
  | "mode"
  | "thoughtLevel"
  | "task"
  | "reply"
  | "stop"
  | "message"
  | "approve";

interface BindCodeRecord {
  botId: string;
  code: string;
  allowedWorkspaces: string[];
  expiresAt: number;
}

interface BotModelOption {
  id: string;
  label: string;
  description?: string;
}

interface BotModelProviderOption {
  id: string;
  label: string;
  description?: string;
  models: BotModelOption[];
}

interface BotTaskSelectionEntry {
  task: ModeTaskMeta;
  workspacePath: string;
  workspaceIdentity?: string;
}

interface BotWorkspaceSelectionEntry {
  workspace: BotWorkspaceRef;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function readNestedRecord(
  value: Record<string, unknown> | null | undefined,
  key: string,
): Record<string, unknown> | null {
  const nested = value?.[key];
  return isRecord(nested) ? nested : null;
}

function readNestedString(
  value: Record<string, unknown> | null | undefined,
  key: string,
): string | undefined {
  const nested = value?.[key];
  return typeof nested === "string" && nested.trim().length > 0 ? nested : undefined;
}

function summarizeCallbackPayload(payload: unknown): string {
  if (!isRecord(payload)) {
    return `type=${typeof payload}`;
  }
  const header = readNestedRecord(payload, "header");
  const event = readNestedRecord(payload, "event") ?? payload;
  const message = readNestedRecord(event, "message");
  const context = readNestedRecord(payload, "context");
  const action = readNestedRecord(payload, "action");
  const keys = Object.keys(payload).slice(0, 16).join(",");
  return [
    `keys=${keys || "none"}`,
    `botId=${readNestedString(payload, "botId") ?? "none"}`,
    `eventType=${readNestedString(header, "event_type") ?? readNestedString(header, "type") ?? readNestedString(payload, "event_type") ?? "none"}`,
    `messageType=${readNestedString(message, "message_type") ?? "none"}`,
    `chatType=${readNestedString(message, "chat_type") ?? readNestedString(context, "chat_type") ?? readNestedString(payload, "chat_type") ?? "none"}`,
    `hasAction=${action ? "true" : "false"}`,
  ].join(" ");
}

function createCode(): string {
  return randomBytes(3).toString("hex").toUpperCase();
}

function normalizeText(value: string): string {
  return value.trim().toLowerCase();
}

function getReplyGranularityOptions(locale: Locale | undefined, provider?: BotProvider) {
  const messageLocale = locale === "en-US" ? "en-US" : "zh-CN";
  const supportedIds = provider ? new Set(getSupportedBotReplyGranularities(provider)) : null;
  return BOT_REPLY_GRANULARITY_OPTIONS.filter(
    (option) => !supportedIds || supportedIds.has(option.id),
  ).map((option) => ({
    id: option.id,
    label: option.label[messageLocale],
  }));
}

function resolveReplyGranularityByValue(
  value: string,
  locale: Locale | undefined,
  provider?: BotProvider,
) {
  const trimmed = value.trim();
  const index = Number.parseInt(trimmed, 10);
  const options = getReplyGranularityOptions(locale, provider);
  if (Number.isFinite(index) && index > 0) {
    return options[index - 1] ?? null;
  }
  const normalized = normalizeText(trimmed);
  const option = BOT_REPLY_GRANULARITY_OPTIONS.find(
    (item) =>
      (item.aliases as readonly string[]).includes(normalized) ||
      normalizeText(item.label["zh-CN"]) === normalized ||
      normalizeText(item.label["en-US"]) === normalized,
  );
  return option ? (options.find((item) => item.id === option.id) ?? null) : null;
}

function resolveOptionByValue<T extends { id: string; label: string }>(
  items: T[],
  value: string,
): T | null {
  const trimmed = value.trim();
  if (/^[1-9]\d*$/u.test(trimmed)) {
    const index = Number.parseInt(trimmed, 10);
    return items[index - 1] ?? null;
  }
  const normalized = normalizeText(trimmed);
  return (
    items.find(
      (item) => normalizeText(item.id) === normalized || normalizeText(item.label) === normalized,
    ) ?? null
  );
}

function isSelectionIndexValue(value: string): boolean {
  return /^[1-9]\d*$/u.test(value.trim());
}

function createOutbound(
  actor: BotActor,
  text: string,
  selection?: SelectionPrompt,
  extras: Pick<BotOutboundMessage, "elicitation" | "locale"> = {},
): BotOutboundMessage {
  return {
    botId: actor.botId,
    provider: actor.provider,
    providerUserId: actor.chatId ?? actor.providerUserId,
    text,
    ...(selection ? { selection } : {}),
    ...extras,
    ...(actor.providerContextToken ? { providerContextToken: actor.providerContextToken } : {}),
  };
}

function resolveAutomationBotDeliveryTarget(
  actor: BotActor,
): ModeAutomationBotDeliveryTarget | undefined {
  // 具备主动推送能力的平台：飞书/Lark、微信、Telegram、企业微信智能机器人。
  // webhook 是入站协议，不作为投递目标。
  // 注意：用直接条件而不是布尔别名，TS 需要它来完成联合类型窄化。
  if (
    actor.provider !== "feishu" &&
    actor.provider !== "lark" &&
    actor.provider !== "weixin" &&
    actor.provider !== "telegram" &&
    actor.provider !== "wecom"
  ) {
    return undefined;
  }
  const providerUserId = actor.chatId?.trim() || actor.providerUserId.trim();
  if (!providerUserId) return undefined;
  return {
    provider: actor.provider,
    botId: actor.botId,
    providerUserId,
    chatType: actor.chatType,
  };
}


function formatSelectionFallback(selection: SelectionPrompt, locale?: Locale): string {
  const lines = selection.options.map((option, index) => {
    const description = option.description ? ` ${option.description}` : "";
    return `${index + 1}. ${option.label}${description}`;
  });
  // Bugfix: 微信这类纯文本通道没有原生选项卡，之前把完整 slash command 和长路径展开，
  // workspace/remote identity 会把消息刷得很长。这里只展示编号，数字解析仍走 pending selection。
  if (selection.showCancel === false) {
    return `${selection.title}\n${lines.join("\n")}\n\n${formatBotMessage(locale, "selectionTextHintNoCancel")}`;
  }
  const cancelLabel = selection.cancelLabel ?? formatBotMessage(locale, "selectionCancelOption");
  return `${selection.title}\n0. ${cancelLabel}\n${lines.join("\n")}\n\n${formatBotMessage(locale, "selectionTextHint")}`;
}

type BotPermissionOptionDisplayKind =
  | "allowOnce"
  | "allowAlways"
  | "rejectOnce"
  | "rejectAlways"
  | "custom";

const BOT_PERMISSION_OPTION_PRIORITY = {
  allowOnce: 0,
  allowAlways: 1,
  rejectOnce: 2,
  rejectAlways: 3,
  custom: 4,
} as const satisfies Record<BotPermissionOptionDisplayKind, number>;

function getBotPermissionOptionDisplayKind(
  option: ModePermissionOption,
): BotPermissionOptionDisplayKind {
  const text = `${option.optionId} ${option.kind} ${option.name}`.toLowerCase();
  const isAlways =
    /\b(always|persistent|permanent|remember)\b/u.test(text) ||
    /始终|永久|记住|不再询问/u.test(text);
  const isAllow = /\b(allow|approve|accept|yes)\b/u.test(text) || /允许|同意|批准/u.test(text);
  const isReject = /\b(deny|reject|decline|no)\b/u.test(text) || /拒绝|不允许|否/u.test(text);
  if (isAllow) {
    return isAlways ? "allowAlways" : "allowOnce";
  }
  if (isReject) {
    return isAlways ? "rejectAlways" : "rejectOnce";
  }
  return "custom";
}

function sortBotPermissionOptions(
  options: readonly ModePermissionOption[],
): ModePermissionOption[] {
  return [...options].sort((left, right) => {
    const leftPriority = BOT_PERMISSION_OPTION_PRIORITY[getBotPermissionOptionDisplayKind(left)];
    const rightPriority = BOT_PERMISSION_OPTION_PRIORITY[getBotPermissionOptionDisplayKind(right)];
    return leftPriority - rightPriority;
  });
}

function formatBotPermissionOptionLabel(option: ModePermissionOption, locale?: Locale): string {
  const displayKind = getBotPermissionOptionDisplayKind(option);
  if (locale === "en-US") {
    switch (displayKind) {
      case "allowOnce":
        return "Allow";
      case "allowAlways":
        return "Always Allow";
      case "rejectOnce":
        return "Deny";
      case "rejectAlways":
        return "Always Deny";
      case "custom":
        return option.name;
    }
  }
  switch (displayKind) {
    case "allowOnce":
      return "允许";
    case "allowAlways":
      return "始终允许";
    case "rejectOnce":
      return "拒绝";
    case "rejectAlways":
      return "始终拒绝";
    case "custom":
      return option.name;
  }
}

function formatBotPermissionOptionDescription(
  option: ModePermissionOption,
  request: Pick<ModePermissionRequest, "title" | "description" | "kind" | "raw">,
  locale?: Locale,
): string | undefined {
  const displayKind = getBotPermissionOptionDisplayKind(option);
  if (displayKind === "custom") {
    return option.kind;
  }
  const scope = getPermissionRequestPreview(request).scope;
  if (locale === "en-US") {
    if (displayKind === "allowOnce") {
      return "Allow this time only";
    }
    if (displayKind === "rejectOnce") {
      return "Reject this time";
    }
    if (displayKind === "allowAlways") {
      return scope === "command"
        ? "Do not ask again for the same command"
        : scope === "file"
          ? "Do not ask again for the same file operation"
          : "Do not ask again for the same permission request";
    }
    return scope === "command"
      ? "Always reject the same command"
      : scope === "file"
        ? "Always reject the same file operation"
        : "Always reject the same permission request";
  }
  if (displayKind === "allowOnce") {
    return "仅允许这一次";
  }
  if (displayKind === "rejectOnce") {
    return "这次先拒绝";
  }
  if (displayKind === "allowAlways") {
    return scope === "command"
      ? "后续相同命令不再询问"
      : scope === "file"
        ? "后续相同文件操作不再询问"
        : "后续相同权限请求不再询问";
  }
  return scope === "command"
    ? "后续相同命令也会直接拒绝"
    : scope === "file"
      ? "后续相同文件操作也会直接拒绝"
      : "后续相同权限请求也会直接拒绝";
}

function isBotPermissionRejectOption(option: ModePermissionOption): boolean {
  const displayKind = getBotPermissionOptionDisplayKind(option);
  return displayKind === "rejectOnce" || displayKind === "rejectAlways";
}

function stripModelProviderDescriptionsForTextSelection(
  selection: SelectionPrompt,
): SelectionPrompt {
  if (selection.action !== "model.provider.set") {
    return selection;
  }
  return {
    ...selection,
    options: selection.options.map((option) => ({
      ...option,
      description: undefined,
    })),
  };
}

function formatWorkspaceOptionLabel(workspace: BotWorkspaceRef, locale?: Locale): string {
  if (!workspace.workspaceIdentity) {
    return workspace.label;
  }
  const remoteLabel = locale === "en-US" ? "[Remote]" : "[远端]";
  return `${workspace.label} ${remoteLabel}`;
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * 任务流订阅 key。必须带 botId：一个会话可以同时绑定多个 bot（同一工作区多机器人），
 * 只按 workspace+task 记键会让后绑定的 bot 被当成重复订阅而拿不到流事件。
 */
/** 打字指示器 key：同一会话可被多个 bot 服务，必须按 bot 分开记。 */
function buildTypingKey(taskId: string, botId: string): string {
  return [taskId, botId].join("::");
}

function buildTaskStreamSubscriptionKey(
  workspacePath: string,
  workspaceIdentity: string | undefined,
  taskId: string,
  botId: string,
): string {
  return [getWorkspaceKey(workspacePath, workspaceIdentity), taskId, botId].join("::");
}

const DEFAULT_BOT_MODE_PROVIDER: ModeProvider = MODE_AGENT_PROVIDER;
// Bot 模式硬锁 yolo：所有 bot task 一律免交互权限，且禁止通过 /mode 切换运行模式。
const BOT_FORCED_MODE = "yolo";
const BOT_TYPING_INTERVAL_MS = 4_000;
const BOT_TASK_META_RETRY_DELAYS_MS = [80, 160, 320] as const;
const BOT_WORKSPACE_REFS_CACHE_TTL_MS = 5_000;
const BOT_MAX_ATTACHMENTS_PER_MESSAGE = 4;
const BOT_MAX_ATTACHMENT_SIZE_BYTES = 5 * 1024 * 1024;
const BOT_ATTACHMENT_DOWNLOAD_TIMEOUT_MS = 30_000;
const REMOTE_RECONNECT_DEDUPE_TTL_MS = 3_000;
const REMOTE_RECONNECT_DELIVERY_DEDUPE_TTL_MS = 2 * 60_000;
const BOT_INBOUND_DELIVERY_DEDUPE_TTL_MS = 2 * 60_000;
const BOT_AUTOMATION_DELIVERY_WARNING_TTL_MS = 5 * 60_000;
const BOT_ELICITATION_CUSTOM_OPTION_ID = "__custom__";
const BOT_ELICITATION_SUBMIT_OPTION_ID = "__submit__";
const BOT_ELICITATION_SKIP_OPTION_ID = "__skip__";
const BOT_ELICITATION_FORM_VALUE_PREFIX = "__form__:";

export function createBotsService(
  deps: BotsServiceDeps,
): IBotsService & { disposeAll(): void; disposeAllAndWait(): Promise<void> } {
  const runStartupBackgroundTasks = deps.runStartupBackgroundTasks !== false;
  const repo = new BotsRepo();
  const bindCodes = new Map<string, BindCodeRecord>();
  const automationDeliveryWarningAtByKey = new Map<string, number>();
  const streamSubscriptions = new Map<string, IDisposable>();
  const streamingCardRequestControllers = new Set<AbortController>();
  const transientInteractionCards = new Map<
    string,
    {
      bot: BotConfig;
      taskId: string;
      handle: BotTransientInteractionCardHandle;
    }
  >();
  const typingIntervals = new Map<string, ReturnType<typeof setInterval>>();
  const typingTargets = new Map<string, { bot: BotConfig; target: BotTypingTarget }>();
  const runningTasks = new Set<string>();
  const liveStatusProgressByTaskId = new Map<
    string,
    { kind: "message" | "thought" | "tool"; text: string }
  >();
  const runtimeByBotId = new Map<string, BotRuntimeInfo>();
  const pendingSelectionsByContext = new Map<string, SelectionPrompt>();
  // 只读取任务订阅和运行状态的数量，不暴露消息内容。
  const memoryDiagnostics = registerMemoryDiagnosticsProvider("bots", () => ({
    streamSubs: streamSubscriptions.size,
    runningTasks: runningTasks.size,
    typingIntervals: typingIntervals.size,
    liveStatusProgress: liveStatusProgressByTaskId.size,
  }));
  const pendingTaskSelectionsByContext = new Map<string, Map<string, BotTaskSelectionEntry>>();
  const pendingWorkspaceSelectionsByContext = new Map<
    string,
    Map<string, BotWorkspaceSelectionEntry>
  >();
  const pendingRemoteReconnectsByKey = new Map<string, Promise<BotOutboundMessage[]>>();
  const recentRemoteReconnectAtByKey = new Map<string, number>();
  const recentRemoteReconnectDeliveryAtByKey = new Map<string, number>();
  const recentInboundDeliveryAtByKey = new Map<string, number>();
  const inboundProcessingQueuesByContext = new Map<string, Promise<void>>();
  let botStorageMigrationPromise: Promise<void> | null = null;
  const cachedWorkspaceRefsByKey = new Map<
    string,
    { expiresAt: number; value: BotWorkspaceRef[] }
  >();
  let cachedLocale: Locale | undefined;
  // 企微连接注册表：provider（出站发送/流式）与 channel runtime（长连接维护）共用。
  // 必须在 providers 之前创建，runtime 在其后创建时复用同一实例。
  const wecomConnection = createWeComConnectionRegistry();
  // 群聊历史：未被 @ / 非绑定用户的消息按会话暂存，触发时作为上下文注入。
  const groupHistory = createGroupHistoryBuffer();
  /** 一次性迁移标记：旧绑定表导入（settings.botBindingByWorkspace）与 path→identity key 升级共用。 */
  let workspaceKeyUpgradeChecked = false;
  const providers: Record<BotProvider, BotProviderAdapter | null> = {
    // AstrBot 桥接已下线：字面量仅为历史配置解析保留，没有适配器。
    astrbot: null,
    telegram: createTelegramBotProvider({
      loadCredential: (key) => deps.credentialService.load(key),
    }),
    webhook: createWebhookBotProvider({
      loadCredential: (key) => deps.credentialService.load(key),
    }),
    feishu: createFeishuBotProvider({
      onDeliveryResult,
      loadCredential: (key) => deps.credentialService.load(key),
    }),
    lark: createFeishuBotProvider({
      onDeliveryResult,
      loadCredential: (key) => deps.credentialService.load(key),
    }),
    weixin: createWeixinBotProvider({
      loadCredential: (key) => deps.credentialService.load(key),
    }),
    discord: null,
    wecom: createWeComBotProvider({
      loadCredential: (key) => deps.credentialService.load(key),
      connection: wecomConnection,
    }),
    dingtalk: createDingtalkBotProvider({
      loadCredential: (key) => deps.credentialService.load(key),
    }),
  };
  let service: IBotsService & {
    disposeAll(): void;
    disposeAllAndWait(): Promise<void>;
  };
  let shutdownPromise: Promise<void> | null = null;

  function onDeliveryResult(bot: BotConfig, deliveryError: string | undefined): void {
    // 收消息正常不代表回复已投递，不能把投递错误混成连接错误。
    const current = runtimeByBotId.get(bot.id);
    setRuntimeStatus({
      botId: bot.id,
      provider: bot.provider,
      status: current?.status ?? (bot.enabled ? "idle" : "disabled"),
      deliveryError,
    });
  }

  function setRuntimeStatus(status: BotRuntimeInfo): void {
    runtimeByBotId.set(status.botId, {
      ...runtimeByBotId.get(status.botId),
      ...status,
      lastUpdateAt: Date.now(),
    });
  }

  const statusSink = {
    getRuntimeStatus(botId: string) {
      return runtimeByBotId.get(botId);
    },
    setRuntimeStatus,
  };
  const telegramRuntime = createTelegramChannelRuntime({
    runBackgroundTasks: runStartupBackgroundTasks,
    credentialService: deps.credentialService,
    telegramProvider: providers.telegram,
    logger: botsLogger,
    statusSink,
    ensureBotStorageMigrated,
    readConfig: () => repo.readConfig(),
    readTelegramOffset,
    writeTelegramOffset,
    processProviderCallback,
  });
  const weixinRuntime = createWeixinChannelRuntime({
    runBackgroundTasks: runStartupBackgroundTasks,
    credentialService: deps.credentialService,
    logger: botsLogger,
    statusSink,
    ensureBotStorageMigrated,
    readConfig: () => repo.readConfig(),
    readWeixinGetUpdatesBuf,
    writeWeixinGetUpdatesBuf,
    processProviderCallback,
  });
  const feishuRuntime = createFeishuChannelRuntime({
    runBackgroundTasks: runStartupBackgroundTasks,
    credentialService: deps.credentialService,
    logger: botsLogger,
    statusSink,
    ensureBotStorageMigrated,
    readConfig: () => repo.readConfig(),
    summarizeCallbackPayload,
    processProviderCallback,
  });
  const wecomRuntime = createWeComChannelRuntime({
    runBackgroundTasks: runStartupBackgroundTasks,
    credentialService: deps.credentialService,
    logger: botsLogger,
    statusSink,
    ensureBotStorageMigrated,
    readConfig: () => repo.readConfig(),
    connection: wecomConnection,
    readWecomDedup,
    writeWecomDedup,
    processProviderCallback,
  });

  const dingtalkRuntime = createDingtalkChannelRuntime({
    runBackgroundTasks: runStartupBackgroundTasks,
    credentialService: deps.credentialService,
    logger: botsLogger,
    statusSink,
    ensureBotStorageMigrated,
    readConfig: () => repo.readConfig(),
    readDingtalkDedup,
    writeDingtalkDedup,
    processProviderCallback,
  });

  /** 通道级状态（游标/去重表）：与对话无关，按 bot 唯一。 */
  function ensureBotChannelState(state: BotsStateFile, botId: string): BotChannelState {
    const existing = state.bots[botId];
    if (existing) {
      return existing;
    }
    const created: BotChannelState = { botId, conversations: {}, updatedAt: Date.now() };
    state.bots[botId] = created;
    return created;
  }

  /** 写通道级字段：只认这些字段，绝不在这里伪造对话上下文。 */
  async function patchBotChannelState(
    botId: string,
    patch: Partial<Omit<BotChannelState, "botId" | "conversations">>,
  ): Promise<void> {
    // 锁内原子 RMW：快照覆盖会被并发对话/窗口的写入打断
    // （docs/specs/bot-state-ownership.md 规则 1）。
    await repo.mutateState((state) => {
      const channel = ensureBotChannelState(state, botId);
      Object.assign(channel, patch, { updatedAt: Date.now() });
    });
  }

  async function readTelegramOffset(botId: string): Promise<number | undefined> {
    return (await repo.readState()).bots[botId]?.telegramOffset;
  }

  async function readWecomDedup(
    botId: string,
  ): Promise<Array<{ id: string; at: number }>> {
    return (await repo.readState()).bots[botId]?.wecomRecentMessageIds ?? [];
  }

  /** 钉钉 Stream 回调的 msgId 去重表（与企微同机制，持久化在 bot 状态里）。 */
  async function readDingtalkDedup(
    botId: string,
  ): Promise<Array<{ id: string; at: number }>> {
    return (await repo.readState()).bots[botId]?.dingtalkRecentMessageIds ?? [];
  }

  async function writeDingtalkDedup(
    botId: string,
    entries: Array<{ id: string; at: number }>,
  ): Promise<void> {
    await patchBotChannelState(botId, { dingtalkRecentMessageIds: entries });
  }

  async function writeWecomDedup(
    botId: string,
    entries: Array<{ id: string; at: number }>,
  ): Promise<void> {
    await patchBotChannelState(botId, { wecomRecentMessageIds: entries });
  }

  async function writeTelegramOffset(botId: string, offset: number): Promise<void> {
    await patchBotChannelState(botId, { telegramOffset: offset });
  }

  async function readWeixinGetUpdatesBuf(botId: string): Promise<string | undefined> {
    return (await repo.readState()).bots[botId]?.weixinGetUpdatesBuf;
  }

  async function writeWeixinGetUpdatesBuf(botId: string, buf: string): Promise<void> {
    await patchBotChannelState(botId, { weixinGetUpdatesBuf: buf });
  }

  async function readContext(actor: BotActor, bot: BotConfig): Promise<BotContextState | null> {
    await ensureBotStorageMigrated();
    const state = await repo.readState();
    const conversation = getBotActorConversation(actor);
    const existing = state.bots[bot.id]?.conversations[conversation.key];
    if (existing) {
      // 绑定优先：bot 被绑定到工作区时，上下文必须落在绑定集内。
      // UI 焦点通知会把上下文切到用户正在看的绑定工作区；这里只纠正漂移出绑定集的情况。
      const boundWorkspaces = await resolveBotBoundWorkspaces(bot.id);
      if (boundWorkspaces.length > 0) {
        const currentKey = getWorkspaceKey(existing.workspacePath, existing.workspaceIdentity);
        const pinned = boundWorkspaces.find((ref) => ref.id === currentKey) ?? boundWorkspaces[0]!;
        if (pinned.id === currentKey) {
          return existing;
        }
        // activeTaskId/pending interactions 属于旧工作区，不能跨工作区沿用。
        const pinnedContext: BotContextState = {
          ...existing,
          workspacePath: pinned.workspacePath,
          workspaceIdentity: pinned.workspaceIdentity,
          workspaceId: pinned.id,
          mode: "draft",
          activeTaskId: null,
          pendingPermissionOptions: undefined,
          pendingElicitation: undefined,
          queuedMessages: undefined,
          pendingDeliveryQueue: undefined,
          deliveryNoticeAt: undefined,
        };
        await writeContext(pinnedContext);
        return pinnedContext;
      }
      const latestWorkspaces = await listWorkspaceRefs();
      const canonicalWorkspace = resolveCanonicalContextWorkspace(existing, latestWorkspaces);
      if (!canonicalWorkspace) {
        return existing;
      }
      const currentWorkspaceKey = getWorkspaceKey(
        existing.workspacePath,
        existing.workspaceIdentity,
      );
      const nextWorkspaceId =
        existing.workspaceId && existing.workspaceId !== currentWorkspaceKey
          ? existing.workspaceId
          : canonicalWorkspace.id;
      const nextContext: BotContextState = {
        ...existing,
        workspacePath: canonicalWorkspace.workspacePath,
        workspaceIdentity: canonicalWorkspace.workspaceIdentity,
        workspaceId: nextWorkspaceId,
      };
      if (
        nextContext.workspacePath === existing.workspacePath &&
        nextContext.workspaceIdentity === existing.workspaceIdentity &&
        nextContext.workspaceId === existing.workspaceId
      ) {
        return existing;
      }
      // Bugfix: 历史 Bot context 可能只有 workspacePath，没有持久化 remote workspaceIdentity。
      // 这样 createTask 虽然还能成功，但后续 bots:task 广播会因为 identity 不匹配被 UI 丢弃，
      // 最终表现成“第三方会话正常回复，侧栏任务列表却不刷新”。这里优先在服务层自愈旧 context。
      await writeContext(nextContext);
      return nextContext;
    }
    const workspace =
      (await resolveBotPinnedWorkspace(bot)) ??
      firstAllowedWorkspace(await listWorkspaceRefs(), bot);
    if (!workspace) {
      return null;
    }
    return {
      botId: bot.id,
      conversationKey: conversation.key,
      conversationKind: conversation.kind,
      conversationId: conversation.id,
      ...(actor.displayName?.trim() ? { conversationLabel: actor.displayName.trim() } : {}),
      workspacePath: workspace.workspacePath,
      workspaceIdentity: workspace.workspaceIdentity,
      workspaceId: workspace.id,
      mode: "draft",
      activeTaskId: null,
      draftOptions: await buildInitializedDraftOptions(workspace),
      updatedAt: Date.now(),
    };
  }

  async function writeContext(context: BotContextState): Promise<void> {
    // 修复原因：快照覆盖写在两次文件锁之间会被其他对话/窗口的写入打断，
    // 改用锁内原子 RMW；撤流订阅与停 typing 是副作用，挪到锁外执行
    // （docs/specs/bot-state-ownership.md 规则 1/3）。
    // 一个对话只能绑定一个会话：该对话离开旧会话时撤掉旧会话的流订阅，
    // 否则旧会话的助手回复会继续推到 IM（表现为"绑定新会话后旧会话还在同步"）。
    // 每个对话各写各的槽位：其他对话的订阅与排队消息不受影响（多会话并发的前提）。
    let disposeTarget:
      | { workspacePath: string; workspaceIdentity: string | undefined; activeTaskId: string }
      | undefined;
    await repo.mutateState((state) => {
      const channel = ensureBotChannelState(state, context.botId);
      const previous = channel.conversations[context.conversationKey];
      if (
        previous?.activeTaskId &&
        (previous.activeTaskId !== context.activeTaskId ||
          getWorkspaceKey(previous.workspacePath, previous.workspaceIdentity) !==
            getWorkspaceKey(context.workspacePath, context.workspaceIdentity))
      ) {
        disposeTarget = {
          workspacePath: previous.workspacePath,
          workspaceIdentity: previous.workspaceIdentity,
          activeTaskId: previous.activeTaskId,
        };
      }
      channel.conversations[context.conversationKey] = { ...context, updatedAt: Date.now() };
      channel.updatedAt = Date.now();
    });
    if (disposeTarget) {
      const target = disposeTarget;
      disposeTaskStreamSubscription(
        target.workspacePath,
        target.workspaceIdentity,
        target.activeTaskId,
        context.botId,
      );
      stopTyping(target.activeTaskId, context.botId);
    }
  }

  /** 撤掉某个会话上该 bot 的流订阅（换绑/切换/解绑/终态共用）。 */
  function disposeTaskStreamSubscription(
    workspacePath: string,
    workspaceIdentity: string | undefined,
    taskId: string,
    botId: string,
  ): void {
    const key = buildTaskStreamSubscriptionKey(workspacePath, workspaceIdentity, taskId, botId);
    streamSubscriptions.get(key)?.dispose();
    streamSubscriptions.delete(key);
  }

  /** 任务运行中收到的纯文本消息入队；带附件的消息不排队（附件下载是瞬时的，暂存会失效）。 */
  async function queueContextMessage(
    context: BotContextState,
    message: BotInboundMessage,
  ): Promise<EnqueueQueuedMessageResult> {
    // 锁内原子 RMW（docs/specs/bot-state-ownership.md 规则 1）。
    let result: EnqueueQueuedMessageResult | undefined;
    await repo.mutateState((state) => {
      const channel = state.bots[context.botId];
      const existing = channel?.conversations[context.conversationKey];
      if (!channel || !existing) {
        return;
      }
      const enqueued = enqueueQueuedMessage(existing.queuedMessages, {
        text: message.text,
        providerUserId: message.actor.providerUserId,
        displayName: message.actor.displayName,
        chatId: message.actor.chatId,
      });
      result = enqueued;
      channel.conversations[context.conversationKey] = {
        ...existing,
        queuedMessages: enqueued.queue,
        updatedAt: Date.now(),
      };
      channel.updatedAt = Date.now();
    });
    return result ?? { queue: [], position: 1 };
  }

  /**
   * 任务终态后按序投递运行期间排队的消息。每次只投一条：递归的 handleMessage 会把
   * 任务重新标为 running 并建立新的 stream 订阅，该回合终态时再取下一条。
   * 上下文已离开该任务时不投递，避免把旧指令打进用户刚切换的会话。
   */
  async function drainQueuedMessages(bot: BotConfig, taskId: string): Promise<void> {
    for (;;) {
      // 锁内原子 RMW：定位持有该会话的对话 → 守卫 → 出队，一次临界区完成；
      // 重投递 handleMessage 在锁外执行（docs/specs/bot-state-ownership.md 规则 1/3）。
      let context: BotContextState | undefined;
      let queued: BotQueuedMessage | undefined;
      await repo.mutateState((state) => {
        const channel = state.bots[bot.id];
        // 一个 bot 可能同时有多个对话各自绑着会话；这里只处理"当前正持有该会话"的那个对话。
        const conversationKey = Object.keys(channel?.conversations ?? {}).find(
          (key) => channel?.conversations[key]?.activeTaskId === taskId,
        );
        const slot = conversationKey ? channel?.conversations[conversationKey] : undefined;
        if (!channel || !conversationKey || !slot || slot.mode !== "task") {
          return;
        }
        if (runningTasks.has(taskId)) {
          return;
        }
        const { next, message } = dequeueQueuedMessage(slot.queuedMessages);
        if (!message) {
          return;
        }
        channel.conversations[conversationKey] = {
          ...slot,
          queuedMessages: next,
          updatedAt: Date.now(),
        };
        channel.updatedAt = Date.now();
        context = slot;
        queued = message;
      });
      if (!context || !queued) {
        return;
      }
      if (!queued.providerUserId) {
        return;
      }
      const actor: BotActor = {
        provider: bot.provider,
        botId: bot.id,
        providerUserId: queued.providerUserId,
        ...(queued.displayName ? { displayName: queued.displayName } : {}),
        chatType: context.conversationKind === "group" ? "group" : "private",
        ...(queued.chatId
          ? { chatId: queued.chatId }
          : context.conversationKind === "group"
            ? { chatId: context.conversationId }
            : {}),
      };
      try {
        await handleMessage({
          botId: bot.id,
          actor,
          text: queued.text,
          receivedAt: queued.receivedAt,
        });
      } catch (error) {
        botsLogger.warn(
          undefined,
          `queued message drain failed bot=${bot.id}: ${
            error instanceof Error ? error.message : String(error)
          }`,
        );
        return;
      }
    }
  }

  // ===== 工作区 → bot 绑定（持久化在 bot-bindings.v3.json）=====

  /**
   * 绑定数据一次性升级（每进程一次；docs/specs/im-bot-remote-workspace-binding.md）：
   * 1) 旧版本把绑定写在 AppSettings.botBindingByWorkspace——绑定表为空时导入一次；
   * 2) settings 远端条目补齐 identity 后，把存量 path-only 绑定 key 升级到 identity key
   *    （保守规则见 resolveLegacyBindingWorkspaceKey），并用同一份工作区候选归一
   *    allowedWorkspaces——否则会话绑定资格按 identity 查询永远看不到存量绑定，
   *    UI 表现为「该机器人不属于当前工作区，无法绑定」。
   * 升级失败不阻塞读取（告警后本次跳过，复位标记让下次读取重试），与旧迁移的容错语义一致。
   */
  async function ensureBotWorkspaceKeysUpgraded(): Promise<void> {
    if (workspaceKeyUpgradeChecked) {
      return;
    }
    workspaceKeyUpgradeChecked = true;
    try {
      const file = await repo.readBindings();
      let bindings = normalizeBotWorkspaceBindings(file.bindings);
      let bindingsChanged = false;
      if (Object.keys(bindings).length === 0 && deps.settingService) {
        const legacy = (await deps.settingService.get().catch(() => null))?.botBindingByWorkspace;
        const migrated = normalizeBotWorkspaceBindings(legacy);
        if (Object.keys(migrated).length > 0) {
          bindings = migrated;
          bindingsChanged = true;
        }
      }
      const workspaces = await listWorkspaceRefs();
      const rekeyed: BotWorkspaceBindings = {};
      for (const [workspaceKey, botIds] of Object.entries(bindings)) {
        const canonicalKey = resolveLegacyBindingWorkspaceKey(workspaceKey, workspaces);
        if (canonicalKey !== workspaceKey) {
          bindingsChanged = true;
        }
        rekeyed[canonicalKey] = [...new Set([...(rekeyed[canonicalKey] ?? []), ...botIds])];
      }
      if (bindingsChanged) {
        await writeBotBindings(rekeyed);
      }
      const config = await repo.readConfig();
      let configChanged = false;
      const bots = config.bots.map((bot) => {
        const allowedWorkspaces = normalizeConfiguredAllowedWorkspaces(
          bot.allowedWorkspaces,
          workspaces,
        );
        if (allowedWorkspaces.join("\n") === bot.allowedWorkspaces.join("\n")) {
          return bot;
        }
        configChanged = true;
        return { ...bot, allowedWorkspaces };
      });
      if (configChanged) {
        await repo.writeConfig({ ...config, bots });
      }
    } catch (error) {
      // 升级失败不能让绑定读写整体挂掉：本次按原数据服务，复位标记让下次读取重试。
      workspaceKeyUpgradeChecked = false;
      botsLogger.warn(
        undefined,
        `bot workspace key upgrade failed: ${error instanceof Error ? error.message : String(error)}`,
      );
    }
  }

  /**
   * 绑定表存放在 bot-bindings.v3.json（repo 自带文件锁 + 原子写），一个工作区可绑定多个 bot。
   * 读侧兼容 v1 的单 bot 字符串形式；旧迁移（settings 导入 + path→identity 升级）收口在
   * ensureBotWorkspaceKeysUpgraded，读取前先跑一次。
   */
  async function readBotBindings(): Promise<BotWorkspaceBindings> {
    await ensureBotWorkspaceKeysUpgraded();
    const file = await repo.readBindings();
    return pruneUnknownBotBindings(normalizeBotWorkspaceBindings(file.bindings));
  }

  /**
   * 绑定表自愈：bot 被删除后表里可能残留它的 id（旧版本删除不清理，或删除中途崩溃）。
   * 指向已不存在 bot 的条目对 UI 只会变成"幽灵机器人"（显示裸 id、挡住空态引导），
   * 这里按当前配置统一清掉；有变化时落盘。
   */
  async function pruneUnknownBotBindings(
    bindings: BotWorkspaceBindings,
  ): Promise<BotWorkspaceBindings> {
    if (Object.keys(bindings).length === 0) {
      return bindings;
    }
    const config = await repo.readConfig().catch(() => null);
    if (!config) {
      // 配置读不到时不动绑定表（可能只是暂时性 IO 失败，清表会把用户绑定一起弄丢）。
      return bindings;
    }
    const knownBotIds = new Set(config.bots.map((bot) => bot.id));
    let next: BotWorkspaceBindings | null = null;
    const boundWorkspacesByBot = new Map<string, string>();
    for (const [workspaceKey, botIds] of Object.entries(bindings)) {
      // 一个 bot 只能绑一个工作区：历史数据里出现在多个工作区的，保留第一次出现的那条。
      const kept = botIds.filter((botId) => {
        if (!knownBotIds.has(botId)) {
          return false;
        }
        if (boundWorkspacesByBot.has(botId)) {
          return false;
        }
        boundWorkspacesByBot.set(botId, workspaceKey);
        return true;
      });
      if (kept.length === botIds.length) {
        continue;
      }
      next ??= { ...bindings };
      if (kept.length === 0) {
        delete next[workspaceKey];
      } else {
        next[workspaceKey] = kept;
      }
    }
    if (!next) {
      return bindings;
    }
    await writeBotBindings(next).catch(() => undefined);
    return next;
  }

  async function writeBotBindings(bindings: BotWorkspaceBindings): Promise<void> {
    await repo.writeBindings({ version: BOT_BINDINGS_FILE_VERSION, bindings });
  }

  /** bot 当前被绑定的已知工作区；绑定 key 指向的工作区已从列表消失时自愈为未绑定。 */
  async function resolveBotBoundWorkspaces(botId: string): Promise<BotWorkspaceRef[]> {
    const [bindings, refs] = await Promise.all([readBotBindings(), listWorkspaceRefs()]);
    return resolveBoundWorkspaceRefs(bindings, botId, refs);
  }

  /**
   * 把 bot 的 allowedWorkspaces 收敛为它的全部绑定工作区（绑定即授权）。
   * 绑定集为空时保持原值：不能让"没有任何绑定"被 normalize 成 "*"（等于放开全部）。
   */
  async function convergeBotAllowedWorkspaces(
    bot: BotConfig,
    bindings: BotWorkspaceBindings,
  ): Promise<void> {
    const boundKeys = Object.entries(bindings)
      .filter(([, botIds]) => botIds.includes(bot.id))
      .map(([workspaceKey]) => workspaceKey.trim())
      .filter(Boolean);
    if (boundKeys.length === 0) {
      return;
    }
    const nextAllowed = normalizeAllowedWorkspaces(boundKeys);
    if (JSON.stringify(nextAllowed) === JSON.stringify(bot.allowedWorkspaces)) {
      return;
    }
    await service.saveBot({ bot: { ...bot, allowedWorkspaces: nextAllowed } });
  }

  /** bot 被绑定时返回钉定工作区（多绑定取第一个已知项，UI 焦点通知会校正）；未绑定返回 null。 */
  async function resolveBotPinnedWorkspace(bot: BotConfig): Promise<BotWorkspaceRef | null> {
    if (!deps.settingService) {
      return null;
    }
    const bound = await resolveBotBoundWorkspaces(bot.id);
    return bound[0] ?? null;
  }

  /**
   * UI 工作区/会话焦点同步入口。只影响绑定到该工作区的 bot——未绑定的 bot 保持
   * 自由切换语义，不被 UI 焦点劫持。
   *
   * 关键约束：**不改写已有的会话绑定**。会话绑定（activeTaskId）是显式状态（右键菜单 /
   * 标题栏「···」/ IM 侧 /task），如果桌面切换会话就把它挪走，绿点会跟着选中跑，
   * 用户会看到"绑定的会话只在选中时才显示"。这里只把**没有会话绑定的 bot**（草稿）
   * 钉到当前工作区，保证它的下一条 IM 消息落在这个工作区；已有绑定的 bot 原地不动。
   */
  async function applyUiFocus(params: BotUiFocusParams): Promise<void> {
    if (!deps.settingService) {
      return;
    }
    const workspaceKey = getWorkspaceKey(params.workspacePath, params.workspaceIdentity);
    const bindings = await readBotBindings();
    const botIds = getWorkspaceBoundBots(bindings, workspaceKey);
    if (botIds.length === 0) {
      return;
    }
    const config = await repo.readConfig();
    const state = await repo.readState();
    // 一个工作区可以绑定多个 bot，各自维护自己的上下文与投递目标。
    // 不做 allowedWorkspaces 检查：botId 就是从该工作区的绑定表里解析出来的，
    // 绑定即授权；这里再查 ACL 只会让收敛失败的旧数据把绑定功能一起挡掉。
    for (const botId of botIds) {
      const bot = findBot(config, botId);
      if (!bot || !bot.enabled) {
        continue;
      }
      // 每个对话各自判断：已绑定会话的对话不被焦点夺走（绿点稳定），
      // 仍处于草稿的对话跟随用户当前工作区，保证下一条 IM 消息落在正确的项目里。
      for (const conversation of listConversations(state, botId)) {
        if (conversation.activeTaskId) {
          continue;
        }
        await focusBotOnWorkspace(
          botId,
          conversation.conversationKey,
          params.workspacePath,
          params.workspaceIdentity,
          null,
        );
      }
    }
  }

  /**
   * 删除 bot 时撤掉它的运行期绑定：流订阅、打字指示、临时交互卡片、挂起的选择态。
   * 否则删除后旧订阅仍会按老 bot 配置往渠道发消息（例如已失效 token 的 Telegram）。
   */
  function disposeBotRuntimeBindings(botId: string): void {
    const suffix = `::${botId}`;
    // 遍历时删除当前项是 Map 的合法用法（规范保证可见条目集合的迭代语义），无需先复制。
    for (const [key, subscription] of streamSubscriptions) {
      if (!key.endsWith(suffix)) {
        continue;
      }
      subscription.dispose();
      streamSubscriptions.delete(key);
    }
    for (const [key, intervalId] of typingIntervals) {
      if (!key.endsWith(suffix)) {
        continue;
      }
      clearInterval(intervalId);
      typingIntervals.delete(key);
    }
    for (const [key, typing] of typingTargets) {
      if (!key.endsWith(suffix)) {
        continue;
      }
      const adapter = providers[typing.bot.provider];
      void adapter?.stopTyping?.(typing.bot, typing.target).catch(() => undefined);
      typingTargets.delete(key);
    }
    for (const [key, card] of transientInteractionCards) {
      if (card.bot.id === botId) {
        transientInteractionCards.delete(key);
      }
    }
    clearPendingSelectionsForBot(botId);
  }

  /** 删除 bot 时把它从所有工作区绑定表里摘掉（该工作区没有其他 bot 时整条记录删除）。 */
  async function pruneBotFromWorkspaceBindings(botId: string): Promise<void> {
    const bindings = await readBotBindings();
    const boundKeys = Object.entries(bindings)
      .filter(([, botIds]) => botIds.includes(botId))
      .map(([workspaceKey]) => workspaceKey);
    if (boundKeys.length === 0) {
      return;
    }
    let next = bindings;
    for (const workspaceKey of boundKeys) {
      next = removeWorkspaceBinding(next, workspaceKey, botId);
    }
    await writeBotBindings(next);
  }

  /**
   * 把 bot 上下文切换到指定工作区（绑定动作 / UI 焦点共用）。
   * taskId 为 null 时进入该工作区的草稿；上下文已在目标时幂等返回；
   * 任务运行中拒绝切换，与 /task.set 的运行中保护保持一致。
   */
  async function focusBotOnWorkspace(
    botId: string,
    conversationKey: string,
    workspacePath: string,
    workspaceIdentity: string | undefined,
    taskId: string | null,
  ): Promise<void> {
    const state = await repo.readState();
    const context = readConversation(state, botId, conversationKey);
    if (context) {
      const sameWorkspace =
        getWorkspaceKey(context.workspacePath, context.workspaceIdentity) ===
        getWorkspaceKey(workspacePath, workspaceIdentity);
      const sameTask = taskId ? context.activeTaskId === taskId : true;
      if (sameWorkspace && sameTask) {
        // 幂等短路：UI↔bot 双向联动的防循环核心
        return;
      }
      if (context.activeTaskId && (await isContextActiveTaskRunning(context))) {
        botsLogger.info(
          undefined,
          `ui focus ignored while task running bot=${botId} task=${context.activeTaskId}`,
        );
        return;
      }
    }
    const config = await repo.readConfig();
    const bot = findBot(config, botId);
    if (!bot) {
      return;
    }
    const identity = buildConversationIdentity(conversationKey, context);
    if (taskId) {
      const nextContext: BotContextState = {
        ...(context ?? { botId, workspacePath, workspaceIdentity, workspaceId: undefined }),
        ...identity,
        botId,
        workspacePath,
        ...(workspaceIdentity ? { workspaceIdentity } : {}),
        workspaceId: getWorkspaceKey(workspacePath, workspaceIdentity),
        mode: "task",
        activeTaskId: taskId,
        // pending interactions 与排队消息属于旧任务，切换时一并清除
        pendingPermissionOptions: undefined,
        pendingElicitation: undefined,
        queuedMessages: undefined,
        pendingDeliveryQueue: undefined,
        deliveryNoticeAt: undefined,
        updatedAt: Date.now(),
      };
      await writeContext(nextContext);
      // UI 焦点带来的任务上下文也要建立流观看，保证 UI 发起的回合实时流转到 IM。
      await ensureContextStreamWatch(bot, nextContext);
      // 换绑/焦点跟随改变了 bot 的会话归属：广播一次让侧栏绿点/右键菜单的绑定投影立刻刷新。
      // source "ui" 表示这是界面自身驱动，消费端只刷新投影、不反向跳转（不会成环）。
      await broadcastTaskListChange(nextContext, taskId, "active_task_changed", {
        source: "ui",
      });
      return;
    }
    if (
      !context ||
      getWorkspaceKey(context.workspacePath, context.workspaceIdentity) !==
        getWorkspaceKey(workspacePath, workspaceIdentity)
    ) {
      await writeDraftContext(
        {
          ...(context ?? { botId }),
          ...identity,
          botId,
          workspacePath,
          ...(workspaceIdentity ? { workspaceIdentity } : {}),
          workspaceId: getWorkspaceKey(workspacePath, workspaceIdentity),
          mode: "draft",
          activeTaskId: null,
          updatedAt: Date.now(),
        },
        await buildInitializedDraftOptions({ workspacePath, workspaceIdentity }),
      );
    }
  }

  /**
   * 出站媒体核心：路径白名单校验 → 大小上限 → 交给 provider 上传。
   * 允许的根目录：当前工作区、应用数据目录、系统临时目录（截图/导出常落在这里）。
   * 用 realpath 解析符号链接后再比较，避免通过链接绕出白名单。
   */
  async function deliverBotMedia(params: {
    bot: BotConfig;
    filePath: string;
    caption?: string;
    chatId?: string;
  }): Promise<void> {
    const adapter = providers[params.bot.provider];
    if (!adapter?.sendMedia) {
      throw new Error("This channel does not support sending files.");
    }
    const state = await repo.readState();
    // 群聊发文件按群对话取工作区；私聊按对方（缺省用绑定用户）取；都取不到时退回最近活跃对话。
    const channel = state.bots[params.bot.id];
    const conversations = listConversations(state, params.bot.id);
    const targetId = params.chatId?.trim();
    const conversation =
      (targetId
        ? conversations.find((item) => item.conversationId === targetId)
        : undefined) ??
      conversations.find(
        (item) =>
          item.conversationKind === "private" &&
          item.conversationId === params.bot.providerUserId?.trim(),
      ) ??
      conversations[0];
    void channel;
    const workspaceRoot = conversation?.workspacePath;
    const rawPath = params.filePath.trim();
    const candidate = isAbsolute(rawPath)
      ? rawPath
      : workspaceRoot
        ? resolvePath(workspaceRoot, rawPath)
        : null;
    if (!candidate) {
      throw new Error("No workspace context to resolve the file path.");
    }
    const real = await realpath(candidate).catch(() => null);
    if (!real) {
      throw new Error(`File not found: ${rawPath}`);
    }
    const allowedRoots = [workspaceRoot, getAppConfigDir(), tmpdir()].filter(
      (root): root is string => Boolean(root?.trim()),
    );
    // 根目录本身也可能是符号链接（macOS 的 /tmp→/private/tmp），realpath 后再比较。
    const resolvedRoots = await Promise.all(
      allowedRoots.map((root) => realpath(root).catch(() => resolvePath(root))),
    );
    if (!resolvedRoots.some((root) => isPathInside(root, real))) {
      throw new Error("Path is outside the allowed directories (workspace / app data / temp).");
    }
    const fileStat = await stat(real);
    if (!fileStat.isFile()) {
      throw new Error("The path is not a regular file.");
    }
    const filename = basename(real);
    const { kind, mimeType } = resolveOutboundMediaKind(filename);
    const limit = kind === "image" ? BOT_MEDIA_IMAGE_LIMIT_BYTES : BOT_MEDIA_FILE_LIMIT_BYTES;
    if (fileStat.size > limit) {
      throw new Error(
        `File is too large (${Math.round(fileStat.size / 1024 / 1024)}MB > ${Math.round(limit / 1024 / 1024)}MB).`,
      );
    }
    const data = await readFile(real);
    const target = params.chatId?.trim() || params.bot.providerUserId?.trim();
    if (!target) {
      throw new Error("Bot has no bound chat to send to.");
    }
    await adapter.sendMedia(
      params.bot,
      { providerUserId: target },
      {
        kind,
        filename,
        mimeType,
        data: new Uint8Array(data),
        ...(params.caption?.trim() ? { caption: params.caption.trim() } : {}),
      },
    );
  }

    /**
   * 主动消息（心跳 / 流订阅 / 桌面镜像）的投递目标：
   * 绑定用户优先；私聊方式=全部用户时回落到最近一次私聊的用户（没有就跳过本轮）。
   */
  function resolveBotProactiveUserId(
    bot: BotConfig,
    conversation: BotConversationState | undefined,
    state?: BotsStateFile,
  ): string | undefined {
    const boundUserId = bot.providerUserId?.trim();
    if (boundUserId) {
      return boundUserId;
    }
    if ((bot.privateChatMode ?? "bound_users") === "all_users") {
      if (conversation?.conversationKind === "private") {
        return conversation.conversationId;
      }
      // 群聊主动投递：回最近说话的私聊用户（原来记在 lastPrivateUserId，现在按对话表推导）。
      const recentPrivate = state
        ? listConversations(state, bot.id).find(
            (item) => item.conversationKind === "private",
          )
        : undefined;
      const lastSeen = recentPrivate?.conversationId?.trim();
      if (lastSeen) {
        return lastSeen;
      }
    }
    return undefined;
  }

  /**
   * 心跳目标对话：优先"绑定了桌面会话"的对话（用户正在桌面用它），其次最近活跃的私聊对话，
   * 再次最近活跃的群对话。心跳是主动打扰，必须打在用户真正在用的那个对话上。
   */
  function pickHeartbeatConversation(
    state: BotsStateFile,
    bot: BotConfig,
  ): BotConversationState | undefined {
    const conversations = listConversations(state, bot.id);
    const bound = conversations.filter(
      (item) => item.mode === "task" && item.activeTaskId,
    );
    const pool = bound.length > 0 ? bound : conversations;
    return (
      pool.find((item) => item.conversationKind === "private") ??
      pool.find((item) => item.conversationKind === "group") ??
      conversations[0]
    );
  }

  /**
   * 心跳回合：合成一条来自绑定用户的消息走正常管线（创建/复用任务、建流、排队），
   * 但以 heartbeat 标记强制摘要模式，且整段回复仅 HEARTBEAT_OK 时保持安静。
   * 工作区存在 HEARTBEAT.md 时把其内容作为检查指引（截断到 4KB）。
   */
  async function runHeartbeatTurn(bot: BotConfig): Promise<void> {
    const state = await repo.readState();
    const context = pickHeartbeatConversation(state, bot);
    const heartbeatUserId = resolveBotProactiveUserId(bot, context, state);
    if (!context || !heartbeatUserId) {
      return;
    }
    await writeHeartbeatTimestamp(bot.id);
    const locale = await readMessageLocale();
    const instructions = await readHeartbeatInstructions(context.workspacePath);
    const prompt =
      locale === "en-US"
        ? [
            `${BOT_HEARTBEAT_PROMPT_MARKER_EN}. Review the current workspace for anything the user should know about.`,
            instructions ? `Follow these project-specific instructions:\n${instructions}` : "",
            "If there is something worth reporting, reply with a concise summary. If everything is fine, reply with exactly HEARTBEAT_OK and nothing else.",
          ]
            .filter(Boolean)
            .join("\n\n")
        : [
            `${BOT_HEARTBEAT_PROMPT_MARKER_ZH}。请查看当前工作区是否有用户需要关注的变化。`,
            instructions ? `按以下项目指引执行：\n${instructions}` : "",
            "有需要汇报的内容时用简洁的语言说明；一切正常时只回复 HEARTBEAT_OK，不要输出其他任何内容。",
          ]
            .filter(Boolean)
            .join("\n\n");
    const actor: BotActor =
      context.conversationKind === "group"
        ? {
            provider: bot.provider,
            botId: bot.id,
            providerUserId: heartbeatUserId,
            chatType: "group",
            chatId: context.conversationId,
            isMention: true,
          }
        : {
            provider: bot.provider,
            botId: bot.id,
            providerUserId: heartbeatUserId,
            chatType: "private",
          };
    // 与用户消息共用同一串行队列：心跳不会与刚落地的用户回合并发写同一任务。
    await enqueueInboundProcessing(actor, async () => {
      await handleMessage({ botId: bot.id, text: prompt, actor }, { heartbeat: true });
    });
  }

  /** 读取工作区 HEARTBEAT.md 作为心跳检查指引（缺失/空文件返回 null；截断 4KB）。 */
  async function readHeartbeatInstructions(workspacePath: string): Promise<string | null> {
    const filePath = join(workspacePath, "HEARTBEAT.md");
    const content = await readFile(filePath, "utf8").catch(() => null);
    const trimmed = content?.trim();
    if (!trimmed) {
      return null;
    }
    return trimmed.length > 4_096 ? trimmed.slice(0, 4_096) : trimmed;
  }

  async function writeHeartbeatTimestamp(botId: string): Promise<void> {
    await patchBotChannelState(botId, { lastHeartbeatAt: Date.now() });
  }

  /** 心跳巡检：60 秒粒度检查间隔；任务运行中或未绑定用户时跳过本轮。 */
  async function tickBotHeartbeats(): Promise<void> {
    const config = await repo.readConfig();
    const now = new Date();
    const state = await repo.readState();
    for (const bot of config.bots) {
      const heartbeat = normalizeBotHeartbeat(bot.heartbeat);
      const context = pickHeartbeatConversation(state, bot);
      if (
        !heartbeat ||
        !bot.enabled ||
        !context ||
        !resolveBotProactiveUserId(bot, context, state)
      ) {
        continue;
      }
      if (
        !isHeartbeatDue(now.getTime(), {
          lastHeartbeatAt: state.bots[bot.id]?.lastHeartbeatAt,
          intervalMinutes: heartbeat.intervalMinutes,
        })
      ) {
        continue;
      }
      if (context.activeTaskId && (await isContextActiveTaskRunning(context))) {
        continue;
      }
      try {
        await runHeartbeatTurn(bot);
      } catch (error) {
        botsLogger.warn(
          undefined,
          `bot heartbeat failed bot=${bot.id}: ${error instanceof Error ? error.message : String(error)}`,
        );
      }
    }
  }

  let heartbeatTimer: ReturnType<typeof setInterval> | null = null;
  function startBotHeartbeatScheduler(): void {
    if (heartbeatTimer) {
      return;
    }
    heartbeatTimer = setInterval(() => {
      void tickBotHeartbeats().catch((error: unknown) => {
        botsLogger.warn(
          undefined,
          `bot heartbeat tick failed: ${error instanceof Error ? error.message : String(error)}`,
        );
      });
    }, 60_000);
    heartbeatTimer.unref?.();
  }

  function stopBotHeartbeatScheduler(): void {
    if (heartbeatTimer) {
      clearInterval(heartbeatTimer);
      heartbeatTimer = null;
    }
  }

/**
   * 确保 bot 上下文指向的任务建立了流观看——不管回合由 IM 还是 UI 发起，
   * AI 的回复都会实时流转到 IM。bot 是单用户模型（providerUserId 即绑定用户），
   * 没有绑定用户就无法确定投递目标，跳过。watchTaskStream 自带订阅去重。
   */
  async function ensureContextStreamWatch(
    bot: BotConfig,
    context: BotContextState,
  ): Promise<void> {
    if (context.mode !== "task" || !context.activeTaskId) {
      return;
    }
    const actor = buildConversationActor(bot, context);
    const user = findBoundUser(bot, actor);
    if (!user) {
      return;
    }
    await watchTaskStream(bot, actor, context, user);
  }

  /**
   * 由对话上下文重建入站 actor：群聊回群、私聊回本人。
   * 群聊用机器人自己的绑定用户身份做授权目标（与入站判定一致）。
   */
  function buildConversationActor(bot: BotConfig, context: BotConversationState): BotActor {
    if (context.conversationKind === "group") {
      return {
        provider: bot.provider,
        botId: bot.id,
        providerUserId: bot.providerUserId?.trim() || context.conversationId,
        chatType: "group",
        chatId: context.conversationId,
        isMention: true,
      };
    }
    return {
      provider: bot.provider,
      botId: bot.id,
      providerUserId: context.conversationId,
      ...(context.conversationLabel ? { displayName: context.conversationLabel } : {}),
      chatType: "private",
    };
  }

  async function writeDraftContext(
    context: BotContextState,
    draftOptions?: BotDraftOptions,
  ): Promise<BotContextState> {
    // Bugfix: 新建草稿状态以前散落在 /new 和 /workspace 分支里，各自手写 activeTaskId=null。
    // workspace 切换后如果还带着旧 task/pending permission，Telegram 权限按钮会命中错误上下文。
    // 这里把“进入新任务草稿”的服务端状态变更收口到同一个 helper，避免跨 workspace 复用旧任务状态。
    const draftContext: BotContextState = {
      ...context,
      mode: "draft",
      activeTaskId: null,
      draftOptions,
      pendingPermissionOptions: undefined,
      pendingElicitation: undefined,
      // 排队消息绑定在旧任务/旧工作区上；进入新草稿意味着用户要重新开始，旧队列一并清除。
      queuedMessages: undefined,
      pendingDeliveryQueue: undefined,
      deliveryNoticeAt: undefined,
    };
    clearPendingSelectionsForBot(context.botId);
    await writeContext(draftContext);
    return draftContext;
  }

  async function handleWeixinFirstActivation(
    message: BotInboundMessage,
    command: BotCommand,
  ): Promise<BotOutboundMessage[] | null> {
    if (message.actor.provider !== "weixin" || command.type !== "message") {
      return null;
    }
    const state = await repo.readState();
    const conversation = getBotActorConversation(message.actor);
    const existingConversation = readConversation(state, message.botId, conversation.key);
    if (
      state.bots[message.botId]?.weixinActivatedAt ||
      existingConversation?.draftOptions ||
      existingConversation?.activeTaskId ||
      existingConversation?.pendingPermissionOptions ||
      existingConversation?.pendingElicitation
    ) {
      return null;
    }
    const auth = await withAuthorizedContext(message, "help");
    if (!auth.ok) {
      return auth.reply;
    }
    // Bugfix: 微信扫码登录只返回 bot token/id，不返回可投递的用户 id。
    // 第一条微信入站消息用于建立会话目标，因此只回激活说明，不把“你好”这类激活文本误当成任务 prompt。
    // 激活标记是通道级（每个 bot 只激活一次），会话上下文由 readContext 正常建立。
    await patchBotChannelState(message.botId, { weixinActivatedAt: Date.now() });
    return [
      createOutbound(
        message.actor,
        // 激活说明保留（它提示了 /帮助 入口），但不再自动附带整份命令清单。
        msg(auth.locale, "weixinActivatedWelcome"),
      ),
    ];
  }

  async function readMessageLocale(): Promise<Locale | undefined> {
    const settings = await deps.settingService?.get().catch(() => null);
    cachedLocale = settings?.locale ?? cachedLocale;
    return cachedLocale;
  }

  function msg(
    locale: Locale | undefined,
    id: BotMessageId,
    values?: Record<string, string | number | undefined>,
  ): string {
    return formatBotMessage(locale, id, values);
  }

  function isSessionExpiredError(error: unknown): boolean {
    const message = error instanceof Error ? error.message : String(error ?? "");
    return /\bSession (not found|is not active):/i.test(message);
  }

  function formatUserFacingBotError(error: unknown, locale: Locale | undefined): string {
    // Bugfix: 旧 bot 消息或脏 task index 会让协议层抛出 Session not found。
    // 直接把 session id 发给用户不可操作；这里保留日志原文，只引导用户新建任务恢复。
    if (isSessionExpiredError(error)) {
      return msg(locale, "sessionExpiredNewTaskHint");
    }
    return error instanceof Error ? error.message : String(error);
  }

  function sanitizeAttachmentFilename(filename: string): string {
    const normalized = Array.from(filename.trim())
      .map((char) => (char.charCodeAt(0) < 32 || '\\/:*?"<>|'.includes(char) ? "_" : char))
      .join("");
    return normalized.length > 0 ? normalized.slice(0, 160) : "attachment";
  }

  function formatAttachmentSize(sizeBytes: number | undefined): string {
    if (!sizeBytes || sizeBytes <= 0) {
      return "unknown size";
    }
    if (sizeBytes >= 1024 * 1024) {
      return `${(sizeBytes / (1024 * 1024)).toFixed(1)}MB`;
    }
    if (sizeBytes >= 1024) {
      return `${Math.ceil(sizeBytes / 1024)}KB`;
    }
    return `${sizeBytes}B`;
  }

  function formatAttachmentRejectedReason(error: unknown, locale: Locale | undefined): string {
    const message = error instanceof Error ? error.message : String(error);
    if (/exceeds 5MB/i.test(message)) {
      return msg(locale, "attachmentTooLarge");
    }
    if (
      /attachment download failed/i.test(message) ||
      /file download failed/i.test(message) ||
      /download .+ failed: HTTP/i.test(message) ||
      /file download timed out/i.test(message) ||
      /attachment download timed out/i.test(message) ||
      /download .+ timed out/i.test(message)
    ) {
      // Bugfix: provider 下载错误会包含 Feishu/Telegram/HTTP 等内部细节，直接回给用户既不友好也不可行动。
      return msg(locale, "attachmentDownloadUnavailable");
    }
    return message;
  }

  function buildAttachmentCachePath(params: {
    botId: string;
    providerMessageId?: string;
    attachment: BotInboundAttachment;
  }): string {
    const messageKey = params.providerMessageId?.trim() || `message-${Date.now()}`;
    const digest = createHash("sha256")
      .update(`${params.botId}:${messageKey}:${params.attachment.id}`)
      .digest("hex")
      .slice(0, 16);
    return join(
      getAppConfigDir(),
      "bot-attachments",
      sanitizeAttachmentFilename(params.botId),
      sanitizeAttachmentFilename(messageKey),
      `${digest}-${sanitizeAttachmentFilename(params.attachment.filename)}`,
    );
  }

  async function fetchAttachmentDownloadUrl(
    attachment: BotInboundAttachment,
  ): Promise<Uint8Array | null> {
    if (!attachment.downloadUrl) {
      return null;
    }
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), BOT_ATTACHMENT_DOWNLOAD_TIMEOUT_MS);
    try {
      const response = await fetch(attachment.downloadUrl, {
        signal: controller.signal,
      });
      if (!response.ok) {
        throw new Error(`download ${attachment.filename} failed: HTTP ${response.status}`);
      }
      return new Uint8Array(await response.arrayBuffer());
    } catch (error) {
      if ((error as { name?: unknown })?.name === "AbortError") {
        // Bugfix: 附件下载卡住时必须尽快失败并回复用户，不能让 bot 回调一直悬挂。
        throw new Error(`download ${attachment.filename} timed out.`);
      }
      throw error;
    } finally {
      clearTimeout(timeout);
    }
  }

  async function resolveAttachmentBytes(
    bot: BotConfig,
    attachment: BotInboundAttachment,
    actor: BotActor,
  ): Promise<{ attachment: BotInboundAttachment; data: Uint8Array } | null> {
    if (attachment.dataBase64) {
      return {
        attachment,
        data: Buffer.from(attachment.dataBase64, "base64"),
      };
    }
    if (attachment.localPath) {
      return {
        attachment,
        data: await readFile(attachment.localPath),
      };
    }
    const provider = providers[bot.provider];
    const downloaded = await provider?.downloadAttachment?.(bot, attachment, actor);
    if (downloaded) {
      return downloaded;
    }
    const fromUrl = await fetchAttachmentDownloadUrl(attachment);
    return fromUrl ? { attachment, data: fromUrl } : null;
  }

  async function cacheResolvedAttachment(params: {
    bot: BotConfig;
    message: BotInboundMessage;
    attachment: BotInboundAttachment;
    data: Uint8Array;
  }): Promise<BotInboundAttachment> {
    const localPath = buildAttachmentCachePath({
      botId: params.bot.id,
      providerMessageId: params.message.actor.providerMessageId,
      attachment: params.attachment,
    });
    await mkdir(dirname(localPath), { recursive: true });
    await writeFile(localPath, params.data);
    return {
      ...params.attachment,
      localPath,
      sizeBytes: params.data.byteLength,
    };
  }

  async function prepareBotMessageContent(
    bot: BotConfig,
    message: BotInboundMessage,
    locale: Locale | undefined,
  ): Promise<PreparedBotMessageContent> {
    const rawAttachments = (message.attachments ?? []).slice(0, BOT_MAX_ATTACHMENTS_PER_MESSAGE);
    const modeAttachments: ModePromptAttachment[] = [];
    const fileLines: string[] = [];
    for (const rawAttachment of rawAttachments) {
      const resolved = await resolveAttachmentBytes(bot, rawAttachment, message.actor);
      if (!resolved) {
        fileLines.push(
          `附件：${rawAttachment.filename} (${rawAttachment.mimeType}, ${formatAttachmentSize(rawAttachment.sizeBytes)})，未能下载。`,
        );
        continue;
      }
      if (resolved.data.byteLength > BOT_MAX_ATTACHMENT_SIZE_BYTES) {
        throw new Error(`${resolved.attachment.filename} exceeds 5MB.`);
      }
      const cached = await cacheResolvedAttachment({
        bot,
        message,
        attachment: resolved.attachment,
        data: resolved.data,
      });
      const dataBase64 = Buffer.from(resolved.data).toString("base64");
      if (cached.kind === "image" || cached.kind === "audio") {
        modeAttachments.push({
          kind: cached.kind,
          filename: cached.filename,
          mimeType: cached.mimeType,
          dataBase64,
          // Bugfix：Bot 已把附件缓存到本地，ModePromptAttachment 也必须携带该路径。
          // 只在 prompt 文本里描述路径会让下游附件策略无法选择本地文件读取。
          localPath: cached.localPath,
        });
        // Bugfix: bot 附件已经被 gateway 下载并缓存到本地。只把图片作为 Mode Agent image block 传入时，
        // 下游 agent 可能把内部临时 URL 再 curl 到 /tmp，导致重复下载、额外权限请求和模型安全拦截。
        // 因此同时把本地缓存路径写进 prompt，明确后续工具操作只能围绕本地文件进行。
        fileLines.push(
          `附件：${cached.filename} (${cached.mimeType}, ${formatAttachmentSize(cached.sizeBytes)})，已作为${cached.kind === "image" ? "图片" : "音频"}输入提供，并保存到：${cached.localPath}。如需读取附件，请直接使用这个本地路径，不要下载或访问临时/远程 URL。`,
        );
        continue;
      }
      fileLines.push(
        `附件：${cached.filename} (${cached.mimeType}, ${formatAttachmentSize(cached.sizeBytes)})，已保存到：${cached.localPath}`,
      );
    }
    const trimmed = message.text.trim();
    const baseContent =
      trimmed || (rawAttachments.length > 0 ? msg(locale, "attachmentOnlyPrompt") : "");
    return {
      content: [baseContent, ...fileLines].filter(Boolean).join("\n\n"),
      modeAttachments,
    };
  }

  function requiresRemoteWorkspaceRuntime(requestedCommand: BotAuthorizedCommand): boolean {
    return (
      requestedCommand !== "help" &&
      requestedCommand !== "status" &&
      requestedCommand !== "workspace" &&
      requestedCommand !== "reconnect" &&
      requestedCommand !== "reply"
    );
  }

  async function isRemoteWorkspaceConnected(
    context: Pick<BotContextState, "workspacePath" | "workspaceIdentity">,
  ): Promise<boolean> {
    if (!context.workspaceIdentity) {
      return true;
    }
    if (!deps.remoteWorkspaceService) {
      // Bugfix: 远端 workspace 没有注入重连服务时，不能默认当作已连接。
      // 否则 Bot 会继续使用缓存模型创建 task，最终在远端 API 层才暴露“模型不存在”等误导性错误。
      return false;
    }
    return deps.remoteWorkspaceService
      .isConnected({
        workspacePath: context.workspacePath,
        workspaceIdentity: context.workspaceIdentity,
      })
      .catch(() => false);
  }

  async function reconnectRemoteWorkspaceForBot(
    context: Pick<BotContextState, "workspacePath" | "workspaceIdentity">,
  ): Promise<BotRemoteWorkspaceReconnectResult> {
    if (!context.workspaceIdentity) {
      return { ok: true };
    }
    if (!deps.remoteWorkspaceService) {
      return {
        ok: false,
        message: "remote reconnect service unavailable",
      };
    }
    return deps.remoteWorkspaceService.ensureConnected({
      workspacePath: context.workspacePath,
      workspaceIdentity: context.workspaceIdentity,
    });
  }

  async function resolveModeTaskServiceForContext(
    context: Pick<BotContextState, "workspacePath" | "workspaceIdentity">,
  ): Promise<IModeTaskService> {
    if (!context.workspaceIdentity) {
      return deps.modeTaskService;
    }
    const remoteModeTaskService = await deps.remoteWorkspaceService?.getModeTaskService?.({
      workspacePath: context.workspacePath,
      workspaceIdentity: context.workspaceIdentity,
    });
    if (remoteModeTaskService) {
      return remoteModeTaskService;
    }
    // Bugfix: 远端 workspace 的 bot 请求不能缺 runtime 时静默走本地 modeTaskService。
    // 否则 /root 这类远端路径会在 macOS/Windows 本地 host 创建任务，模型和文件系统都错位。
    throw new Error(
      `当前远端项目 ${context.workspacePath} runtime 不可用，请发送 **/重连** 后重试。`,
    );
  }

  async function resolveModelSelectionServiceForContext(
    context: Pick<BotContextState, "workspacePath" | "workspaceIdentity">,
  ): Promise<Pick<IModelSelectionService, "getView">> {
    if (!context.workspaceIdentity) return deps.modelSelectionService;
    const service = await deps.remoteWorkspaceService?.getModelSelectionService?.({
      workspacePath: context.workspacePath,
      workspaceIdentity: context.workspaceIdentity,
    });
    if (service) return service;
    throw new Error(
      `当前远端项目 ${context.workspacePath} runtime 不可用，请发送 **/重连** 后重试。`,
    );
  }

  async function blockDisconnectedRemoteWorkspace(params: {
    message: BotInboundMessage;
    context: BotContextState;
    locale: Locale | undefined;
    requestedCommand: BotAuthorizedCommand;
  }): Promise<BotOutboundMessage[] | null> {
    if (
      !params.context.workspaceIdentity ||
      !requiresRemoteWorkspaceRuntime(params.requestedCommand) ||
      (await isRemoteWorkspaceConnected(params.context))
    ) {
      return null;
    }
    // Bugfix: 普通消息、配置修改和权限响应不应该隐式改变远端连接状态。
    // 远端恢复只允许显式 /reconnect 触发，避免同一条消息有时执行、有时只是在后台打开连接。
    return [
      createOutbound(
        params.message.actor,
        msg(params.locale, "remoteDisconnected", {
          workspacePath: params.context.workspacePath,
        }),
      ),
    ];
  }

  function currentOptionSuffix(locale: Locale | undefined): string {
    return locale === "en-US" ? "current" : "当前";
  }

  function formatReplyGranularityLabel(
    id: BotReplyGranularity | undefined,
    locale: Locale | undefined,
    provider?: BotProvider,
  ): string {
    const currentId = provider
      ? normalizeBotReplyGranularity(provider, id)
      : (id ?? getDefaultBotReplyGranularity());
    return (
      getReplyGranularityOptions(locale, provider).find((option) => option.id === currentId)
        ?.label ?? currentId
    );
  }

  function clearCandidateCaches(): void {
    cachedWorkspaceRefsByKey.clear();
  }

  function markCurrentSelection(
    selection: SelectionPrompt,
    locale: Locale | undefined,
  ): SelectionPrompt {
    const cancelLabel = msg(locale, "selectionCancelOption");
    if (!selection.currentId) {
      return { ...selection, cancelLabel };
    }
    const suffix = currentOptionSuffix(locale);
    return {
      ...selection,
      cancelLabel,
      options: selection.options.map((option) =>
        option.id === selection.currentId
          ? { ...option, label: `${option.label} · ${suffix}` }
          : option,
      ),
    };
  }

  async function listUserConfigOptions(
    _params: BotUserConfigOptionsParams,
  ): Promise<ModeConfigOption[]> {
    return [];
  }
  async function ensureBotStorageMigrated(): Promise<void> {
    // 单向导入已收口到 Repo；这里只等待初始化，不再读取旧模型字段或重写当前状态。
    if (!botStorageMigrationPromise) {
      botStorageMigrationPromise = Promise.all([repo.readConfig(), repo.readState()])
        .then(() => rekeyLegacyConversations())
        .catch((error: unknown) => {
          botStorageMigrationPromise = null;
          throw error;
        });
    }
    await botStorageMigrationPromise;
  }

  /**
   * v3 单上下文迁移时无法判定归属的对话会落在 legacy 占位键上。
   * 这里拿到 bot 配置后把绑定用户的私聊对话重映射到真正的键（private:<userId>），
   * 保住用户已有的"桌面会话 ↔ 机器人"绑定与工作区上下文；群聊归属无法从旧状态判定，
   * 首次入站消息会为对应群建立新对话。
   */
  async function rekeyLegacyConversations(): Promise<void> {
    const config = await repo.readConfig();
    // 锁内原子 RMW（docs/specs/bot-state-ownership.md 规则 1）；
    // 无论是否重映射都写一次：把磁盘上的 v3 形状固化成本次进程实际使用的 v4，
    // 避免"内存 v4 / 磁盘 v3"长期并存（迁移是幂等的，重复读不会产生额外改动）。
    await repo.mutateState((state) => {
      for (const bot of config.bots) {
        const channel = state.bots[bot.id];
        const legacy = channel?.conversations[BOT_LEGACY_CONVERSATION_KEY];
        if (!channel || !legacy) {
          continue;
        }
        const boundUserId = bot.providerUserId?.trim();
        const targetKey = boundUserId
          ? makeBotConversationKey("private", boundUserId)
          : undefined;
        delete channel.conversations[BOT_LEGACY_CONVERSATION_KEY];
        if (targetKey && !channel.conversations[targetKey]) {
          channel.conversations[targetKey] = {
            ...legacy,
            conversationKey: targetKey,
            conversationKind: "private",
            conversationId: boundUserId!,
          };
        }
        channel.updatedAt = Date.now();
      }
    });
  }

  /** 由对话键（可能来自 UI 绑定/自动化目标）补齐对话身份字段。 */
  function buildConversationIdentity(
    conversationKey: string,
    existing?: BotConversationState,
    label?: string,
  ): Pick<
    BotConversationState,
    "conversationKey" | "conversationKind" | "conversationId" | "conversationLabel"
  > {
    const parsed =
      parseBotConversationKey(conversationKey) ?? {
        kind: "private" as const,
        id: conversationKey,
      };
    const trimmedLabel = label?.trim() || existing?.conversationLabel?.trim();
    return {
      conversationKey,
      conversationKind: parsed.kind,
      conversationId: parsed.id,
      ...(trimmedLabel ? { conversationLabel: trimmedLabel } : {}),
    };
  }

  /** 某对话的 bot 状态（不存在返回 undefined）。 */
  function readConversation(
    state: BotsStateFile,
    botId: string,
    conversationKey: string,
  ): BotConversationState | undefined {
    return state.bots[botId]?.conversations[conversationKey];
  }

  /** 列出该 bot 的所有对话上下文（按最近更新排序）。 */
  function listConversations(
    state: BotsStateFile,
    botId: string,
  ): BotConversationState[] {
    return Object.values(state.bots[botId]?.conversations ?? {}).sort(
      (left, right) => right.updatedAt - left.updatedAt,
    );
  }

  /** 该 bot 持有指定会话（activeTaskId）的全部对话。 */
  function listConversationsByTask(
    state: BotsStateFile,
    botId: string,
    taskId: string,
  ): BotConversationState[] {
    return listConversations(state, botId).filter(
      (conversation) => conversation.activeTaskId === taskId,
    );
  }

  /** 把某 bot 仍处于草稿的所有对话钉到指定工作区（工作区绑定/UI 焦点共用）。 */
  async function focusBotDraftConversationsOnWorkspace(
    botId: string,
    workspacePath: string,
    workspaceIdentity: string | undefined,
  ): Promise<void> {
    const state = await repo.readState();
    for (const conversation of listConversations(state, botId)) {
      if (conversation.activeTaskId) {
        continue;
      }
      await focusBotOnWorkspace(
        botId,
        conversation.conversationKey,
        workspacePath,
        workspaceIdentity,
        null,
      );
    }
  }

  async function listActiveTaskConfigOptions(
    context: Pick<BotContextState, "workspacePath" | "workspaceIdentity">,
    taskId: string,
  ): Promise<ModeConfigOption[]> {
    const modeTaskService = await resolveModeTaskServiceForContext(context);
    return modeTaskService.getTaskConfigOptions({ taskId });
  }

  function findSelectConfigOption(
    options: readonly ModeConfigOption[],
    configId: "model" | "mode" | "thoughtLevel",
  ): (ModeConfigOption & { type: "select" }) | undefined {
    const category = configId === "thoughtLevel" ? "thought_level" : configId;
    return options.find(
      (item): item is ModeConfigOption & { type: "select" } =>
        item.type === "select" && (item.category === category || item.id === category),
    );
  }

  function listConfigSelectOptions(
    options: readonly ModeConfigOption[],
    configId: "model" | "mode" | "thoughtLevel",
    context: { locale?: Locale; provider?: ModeProvider } = {},
  ): BotModelOption[] {
    const option = findSelectConfigOption(options, configId);
    return (option?.options ?? []).map((item) => {
      const baseOption = {
        id: item.value,
        label: item.name,
        description: item.description,
      };
      return {
        ...baseOption,
        // 保持 Bot 与工具栏的模式展示一致。
        label: formatConfigOptionLabel(baseOption, {
          configId,
          locale: context.locale,
          provider: context.provider,
        }),
      };
    });
  }

  function getConfigCommandMissingMessageId(configId: "mode" | "thoughtLevel"): BotMessageId {
    return configId === "mode" ? "modeMissing" : "thoughtLevelMissing";
  }

  function getModeDisplayLabel(
    locale: Locale | undefined,
    provider: ModeProvider | undefined,
    option: Pick<BotModelOption, "id" | "label">,
  ): string {
    if (!provider) {
      return option.label;
    }
    const isEnglish = locale === "en-US";
    const labels: Partial<Record<ModeProvider, Record<string, string>>> = {
      glm: {
        default: isEnglish ? "Default" : "默认",
        yolo: "Yolo",
        plan: isEnglish ? "Plan" : "计划",
      },
    };
    return labels[provider]?.[option.id] ?? option.label;
  }

  function formatConfigOptionLabel(
    option: BotModelOption,
    context: {
      configId: "model" | "mode" | "thoughtLevel";
      locale?: Locale;
      provider?: ModeProvider;
    },
  ): string {
    if (context.configId !== "mode") {
      return option.label;
    }
    return getModeDisplayLabel(context.locale, context.provider, option);
  }

  function createModelSelectionProviderOption(
    provider: ModelSelectionView["providers"][number],
  ): BotModelProviderOption {
    return {
      id: provider.providerId,
      label: provider.providerName?.trim() || provider.providerId,
      models: provider.models.map((model) => ({
        id: encodeCustomModelValue(provider.providerId, model.modelId),
        label: model.modelId,
      })),
    };
  }

  async function readModelSelectionView(
    context: Pick<BotContextState, "workspacePath" | "workspaceIdentity">,
    selection?: ModelSelection,
  ): Promise<ModelSelectionView | null> {
    const service = await resolveModelSelectionServiceForContext(context).catch(() => null);
    if (!service) return null;
    return service.getView
      .call(service, selection ? { selection } : undefined)
      .catch((error: unknown) => {
        botsLogger.warn(
          undefined,
          `read model selection view for bot model display failed: ${error instanceof Error ? error.message : String(error)}`,
        );
        return null;
      });
  }

  async function listModelSelectionProviderOptions(
    context: Pick<BotContextState, "workspacePath" | "workspaceIdentity">,
  ): Promise<BotModelProviderOption[]> {
    const view = await readModelSelectionView(context);
    // 旧缓存没有 workspaceIdentity 隔离，远端断连时会显示其他 Host 的候选。
    // 当前菜单只消费目标 View；失败留空，下一次正常读取即可恢复，不借本地补选。
    if (!view) return [];
    return view.providers
      .map(createModelSelectionProviderOption)
      .filter((provider) => provider.models.length > 0);
  }

  async function listModelProviderOptionsForActiveTask(
    task: Pick<ModeTaskMeta, "model" | "workspacePath" | "workspaceIdentity">,
    _activeProvider: ModeProvider,
  ): Promise<BotModelProviderOption[]> {
    return listModelSelectionProviderOptions(task);
  }

  async function listModelOptionsForProviderFromActiveTask(
    task: Pick<ModeTaskMeta, "model" | "workspacePath" | "workspaceIdentity">,
    activeProvider: ModeProvider,
    providerId: string,
  ): Promise<BotModelOption[]> {
    return (
      (await listModelProviderOptionsForActiveTask(task, activeProvider)).find(
        (provider) => provider.id === providerId,
      )?.models ?? []
    );
  }

  function readModelProviderSelectionModels(provider: unknown): BotModelOption[] {
    const models = isRecord(provider) ? provider.models : undefined;
    if (!Array.isArray(models)) {
      return [];
    }
    return models.filter(
      (model): model is BotModelOption =>
        isRecord(model) && typeof model.id === "string" && typeof model.label === "string",
    );
  }

  async function listAllModelOptionsForActiveTask(
    task: Pick<ModeTaskMeta, "model" | "workspacePath" | "workspaceIdentity">,
    activeProvider: ModeProvider,
  ): Promise<BotModelOption[]> {
    return (await listModelProviderOptionsForActiveTask(task, activeProvider)).flatMap(
      (provider) => provider.models,
    );
  }

  function readCurrentActiveTaskModel(
    task: Pick<ModeTaskMeta, "model">,
    options: readonly ModeConfigOption[],
  ): string | undefined {
    return readConfigSelectCurrentValue(options, "model") ?? task.model;
  }

  async function formatStatusModelLabel(
    model: string | undefined,
    context: Pick<BotContextState, "workspacePath" | "workspaceIdentity">,
  ): Promise<string> {
    if (!model) {
      return "-";
    }
    const customModel = decodeCustomModelValue(model);
    if (customModel?.providerId) {
      const modelSelectionOptions = await listModelSelectionProviderOptions(context);
      const providerLabel = modelSelectionOptions.find(
        (item) => item.id === customModel.providerId,
      )?.label;
      if (providerLabel && customModel.modelName) {
        return `${providerLabel}/${customModel.modelName}`;
      }
      return providerLabel ?? model;
    }
    const separatorIndex = model.indexOf("/");
    if (separatorIndex <= 0 || separatorIndex === model.length - 1) {
      return model;
    }
    const providerId = model.slice(0, separatorIndex);
    const modelName = model.slice(separatorIndex + 1);
    const modelSelectionOptions = await listModelSelectionProviderOptions(context);
    const providerLabel = modelSelectionOptions.find((item) => item.id === providerId)?.label;
    // Bugfix: /status 只应该暴露用户能识别的模型供应商名称。
    // 旧 bot-state 或 task config 可能保存成 providerId/modelId，providerId 对用户没有意义。
    return providerLabel ? `${providerLabel}/${modelName}` : model;
  }

  async function readCurrentModelProviderId(
    task: Pick<ModeTaskMeta, "model" | "workspacePath" | "workspaceIdentity">,
    options: readonly ModeConfigOption[],
    activeProvider: ModeProvider,
  ): Promise<string | undefined> {
    const currentValue = readCurrentActiveTaskModel(task, options);
    if (!currentValue) {
      return undefined;
    }
    const customModel = decodeCustomModelValue(currentValue);
    if (customModel?.providerId) {
      return customModel.providerId;
    }
    return (
      (await listModelProviderOptionsForActiveTask(task, activeProvider)).find((provider) =>
        provider.models.some((model) => model.id === currentValue),
      )?.id ?? getNativeModelProviderId(activeProvider)
    );
  }

  function resolveCustomModelRuntimeModelId(
    _activeProvider: ModeProvider,
    customModel: { providerId: string; modelName?: string },
  ): string | undefined {
    if (!customModel.modelName) {
      return undefined;
    }
    return customModel.modelName;
  }

  function readConfigSelectCurrentValue(
    options: readonly ModeConfigOption[],
    configId: "model" | "mode" | "thoughtLevel",
  ): string | undefined {
    const currentValue = findSelectConfigOption(options, configId)?.currentValue;
    return typeof currentValue === "string" ? currentValue : undefined;
  }

  function resolveSupportedDraftMode(
    options: readonly ModeConfigOption[],
    mode: string | undefined,
    provider: ModeProvider,
  ): string | undefined {
    if (!mode) {
      return undefined;
    }
    return resolveProviderModeIdFromConfigOptions({
      configOptions: options,
      modeId: mode,
      provider,
    })
      ? mode
      : undefined;
  }

  function readConfigSelectCurrentLabel(
    options: readonly ModeConfigOption[],
    configId: "model" | "mode" | "thoughtLevel",
    context: { locale?: Locale; provider?: ModeProvider } = {},
  ): string | undefined {
    const currentValue = readConfigSelectCurrentValue(options, configId);
    if (!currentValue) {
      return undefined;
    }
    return (
      listConfigSelectOptions(options, configId, context).find(
        (option) => option.id === currentValue,
      )?.label ?? currentValue
    );
  }

  function readConfigSelectLabelForValue(
    options: readonly ModeConfigOption[],
    configId: "model" | "mode" | "thoughtLevel",
    value: string | undefined,
    context: { locale?: Locale; provider?: ModeProvider } = {},
  ): string | undefined {
    if (!value) {
      return undefined;
    }
    return (
      listConfigSelectOptions(options, configId, context).find((option) => option.id === value)
        ?.label ?? value
    );
  }

  function readCurrentActiveTaskMode(
    task: Pick<ModeTaskMeta, "mode">,
    options: readonly ModeConfigOption[],
  ): string | undefined {
    return readConfigSelectCurrentValue(options, "mode") ?? task.mode;
  }

  async function listProviderConfigOptionsForActiveTask(
    task: Pick<ModeTaskMeta, "workspacePath" | "workspaceIdentity">,
    activeProvider: ModeProvider,
  ): Promise<ModeConfigOption[]> {
    return listUserConfigOptions({
      workspacePath: task.workspacePath,
      workspaceIdentity: task.workspaceIdentity,
      provider: activeProvider,
    });
  }

  function normalizeBotDraftOptions(draftOptions: BotDraftOptions): BotDraftOptions {
    // Bugfix: bot-state 里可能还残留旧三方 CLI 草稿 provider。
    // 如果直接复用，/new 后首条消息会重新创建第三方 runtime，绕过 Mode Agent 单一事实源。
    return {
      ...draftOptions,
      provider: normalizeAgentProviderToModeAgent(draftOptions.provider),
    };
  }

  async function buildInitializedDraftOptions(
    context: Pick<BotContextState, "workspacePath" | "workspaceIdentity">,
    provider?: ModeProvider,
  ): Promise<BotDraftOptions> {
    const requestedProvider = normalizeAgentProviderToModeAgent(
      provider ?? DEFAULT_BOT_MODE_PROVIDER,
    );
    if (context.workspaceIdentity && !(await isRemoteWorkspaceConnected(context))) {
      // Bugfix: 远端断连时初始化草稿也不能偷偷申请远端 Mode Agent runtime。
      // 只有 /reconnect 能恢复连接；草稿先保留最小默认值，重连成功后再刷新。
      return { provider: requestedProvider };
    }
    const resolvedProvider = requestedProvider;
    return {
      provider: resolvedProvider,
      mode: BOT_FORCED_MODE,
    };
  }

  async function buildActiveTaskDraftOptions(context: BotContextState): Promise<BotDraftOptions> {
    const activeTask = await readContextActiveTaskMeta(context);
    if (!context.activeTaskId || !activeTask?.provider) {
      return buildInitializedDraftOptions(context);
    }
    const configOptions = await listActiveTaskConfigOptions(context, context.activeTaskId).catch(
      () => [],
    );
    const resolvedProvider = normalizeAgentProviderToModeAgent(activeTask.provider);
    // Bot 硬锁 yolo：继承当前 task 时也强制 yolo，不沿用原 task 的 mode。
    const forcedMode = resolveSupportedDraftMode(configOptions, BOT_FORCED_MODE, resolvedProvider);
    const currentModel = readCurrentActiveTaskModel(activeTask, configOptions);
    const parsedSelection = currentModel ? parseBotModelOptionValue(currentModel) : undefined;
    const reasoningLevel = readConfigSelectCurrentValue(configOptions, "thoughtLevel");
    const modelSelection = parsedSelection
      ? {
          ...parsedSelection,
          ...(reasoningLevel ? { options: { reasoningLevel } } : {}),
        }
      : undefined;
    return {
      provider: resolvedProvider,
      ...(modelSelection ? { modelSelection } : {}),
      ...(forcedMode ? { mode: forcedMode } : {}),
    };
  }

  async function ensureDraftOptions(context: BotContextState): Promise<BotDraftOptions> {
    if (context.draftOptions) {
      const normalizedDraftOptions = normalizeBotDraftOptions(context.draftOptions);
      if (normalizedDraftOptions.provider !== context.draftOptions.provider) {
        await writeContext({ ...context, draftOptions: normalizedDraftOptions });
      }
      return normalizedDraftOptions;
    }
    const draftOptions = await buildInitializedDraftOptions(context);
    await writeContext({ ...context, draftOptions });
    return draftOptions;
  }

  async function writeDraftOptions(
    context: BotContextState,
    draftOptions: BotDraftOptions,
  ): Promise<BotContextState> {
    const normalizedDraftOptions = normalizeBotDraftOptions(draftOptions);
    const nextContext: BotContextState = {
      ...context,
      mode: "draft",
      activeTaskId: null,
      draftOptions: normalizedDraftOptions,
    };
    await writeContext(nextContext);
    return nextContext;
  }

  async function resolveDraftOptionsForDisplay(context: BotContextState): Promise<BotDraftOptions> {
    const original = await ensureDraftOptions(context);
    const view = await readModelSelectionView(context, original.modelSelection);
    // 菜单也必须展示派发将使用的身份。这里只返回副本；查看菜单不能写回原草稿。
    return {
      ...original,
      modelSelection:
        (original.modelSelection ? view?.effectiveSelection : view?.preferredSelection) ??
        undefined,
    };
  }

  async function listDraftConfigOptions(
    context: BotContextState,
    draftOptions: BotDraftOptions,
    resolvedView?: ModelSelectionView | null,
  ): Promise<ModeConfigOption[]> {
    const view =
      resolvedView === undefined
        ? await readModelSelectionView(context, draftOptions.modelSelection)
        : resolvedView;
    const selection = draftOptions.modelSelection
      ? view?.effectiveSelection
      : view?.preferredSelection;
    if (!selection) return [];
    const model = view?.providers
      .find((provider) => provider.providerId === selection.providerId)
      ?.models.find((candidate) => candidate.modelId === selection.modelId);
    const spec = model?.config.optionSpecs.reasoningLevel;
    if (!spec) return [];
    return [
      {
        id: "thought_level",
        name: "Reasoning",
        category: "thought_level",
        type: "select",
        currentValue: selection.options?.reasoningLevel ?? "",
        options: spec.values.map((value) => ({ value, name: value })),
      },
    ];
  }

  async function applyDraftConfigOptions(
    context: BotContextState,
    taskId: string,
    traceId: string,
  ): Promise<void> {
    const draftOptions = context.draftOptions;
    if (!draftOptions) {
      return;
    }
    // Bugfix: workspace configOptions 描述的是切换前的工作区模型，不能用来校验新 task 的配置。
    // 例如 GLM 的 enabled 会被误下发给刚切换的 DeepSeek，导致首条微信消息回调失败。
    const configOptions = await listActiveTaskConfigOptions(context, taskId);
    const modeOption = configOptions.find(
      (option) => option.category === "mode" && option.type === "select",
    );
    // Bot 硬锁 yolo：无论草稿/继承的 mode 是什么，建 task 时一律下发 yolo。
    // 这是 mode 真正进入 agent session 的唯一咽喉，保证任何 bot task 都免交互权限。
    const forcedDraftMode = resolveSupportedDraftMode(
      configOptions,
      BOT_FORCED_MODE,
      draftOptions.provider,
    );
    if (modeOption?.id && forcedDraftMode) {
      const modeTaskService = await resolveModeTaskServiceForContext(context);
      await modeTaskService.setMode({
        taskId,
        mode: forcedDraftMode as ModeTaskMode,
      });
    } else if (modeOption?.id) {
      // provider 不支持 yolo（非 Mode Agent）：保持其自身默认模式，避免首条消息回调失败。
      botsLogger.debug(
        traceId,
        `skip forced yolo mode unsupported provider=${draftOptions.provider}`,
      );
    }
  }

  function getActorContextKey(actor: BotActor): string {
    return [actor.botId, actor.provider, actor.chatId?.trim() || actor.providerUserId].join("::");
  }

  function clearPendingSelectionsForBot(botId: string): void {
    const matchesBot = (contextKey: string): boolean =>
      contextKey === botId || contextKey.startsWith(`${botId}::`);
    for (const contextKey of pendingSelectionsByContext.keys()) {
      if (matchesBot(contextKey)) {
        pendingSelectionsByContext.delete(contextKey);
      }
    }
    for (const contextKey of pendingTaskSelectionsByContext.keys()) {
      if (matchesBot(contextKey)) {
        pendingTaskSelectionsByContext.delete(contextKey);
      }
    }
    for (const contextKey of pendingWorkspaceSelectionsByContext.keys()) {
      if (matchesBot(contextKey)) {
        pendingWorkspaceSelectionsByContext.delete(contextKey);
      }
    }
  }

  function resolvePendingSelectionOption(
    actor: BotActor,
    action: SelectionPrompt["action"],
    value: string,
  ): SelectionPrompt["options"][number] | null {
    const actorContextKey = getActorContextKey(actor);
    const selection = pendingSelectionsByContext.get(actorContextKey);
    if (selection?.action !== action) {
      return null;
    }
    const option = resolveOptionByValue(selection.options, value);
    if (option) {
      pendingSelectionsByContext.delete(actorContextKey);
    }
    return option;
  }

  function clearPendingSelection(actor: BotActor): void {
    const actorContextKey = getActorContextKey(actor);
    pendingSelectionsByContext.delete(actorContextKey);
    pendingTaskSelectionsByContext.delete(actorContextKey);
    pendingWorkspaceSelectionsByContext.delete(actorContextKey);
  }

  function resolvePendingSelectionCommand(actor: BotActor, value: string): BotCommand | null {
    const actorContextKey = getActorContextKey(actor);
    const selection = pendingSelectionsByContext.get(actorContextKey);
    if (!selection) {
      return null;
    }
    if (actor.provider !== "weixin") {
      // Bugfix: 只有微信没有结构化选项，只能靠“回复数字”承接 pending selection。
      // Telegram/飞书等 provider 有按钮回调，普通文本不应被隐式解析成菜单选择。
      clearPendingSelection(actor);
      return null;
    }
    if (!isSelectionIndexValue(value)) {
      // Bugfix: /task 等列表命令会留下 pending selection。
      // 旧逻辑允许普通文本按 label 命中选项，用户输入与 task 标题同名的消息时会被误切 task。
      // 隐式选择只接受纯数字；按 id/label 选择仍通过显式 /task <value> 等命令完成。
      clearPendingSelection(actor);
      return null;
    }
    const option = resolveOptionByValue(selection.options, value);
    if (!option) {
      clearPendingSelection(actor);
      return null;
    }
    pendingSelectionsByContext.delete(actorContextKey);
    switch (selection.action) {
      case "workspace.set":
        return { type: "workspace.set", value: option.id };
      case "model.provider.set":
        return { type: "model.provider.set", value: option.id };
      case "model.set":
        return { type: "model.set", value: option.id };
      case "mode.set":
        return { type: "mode.set", value: option.id };
      case "thoughtLevel.set":
        return { type: "thoughtLevel.set", value: option.id };
      case "task.set":
        return { type: "task.set", value: option.id };
      case "reply.set":
        return { type: "reply.set", value: option.id };
      case "permission.respond":
        return { type: "permission.respond", value };
      case "elicitation.respond":
        return { type: "elicitation.respond", value: option.id };
    }
  }

  function shouldUseTransientInteractionCard(bot: BotConfig, user: BotConfig): boolean {
    const adapter = providers[bot.provider];
    return (
      isFeishuBotProvider(bot.provider) &&
      normalizeBotReplyGranularity(bot.provider, user.replyMode) === "streaming_card" &&
      Boolean(adapter?.createTransientInteractionCard) &&
      Boolean(adapter?.updateTransientInteractionCard)
    );
  }

  async function upsertTransientInteractionCard(
    bot: BotConfig,
    actor: BotActor,
    taskId: string,
    message: BotOutboundMessage,
  ): Promise<void> {
    const adapter = providers[bot.provider];
    const key = getActorContextKey(actor);
    const existing = transientInteractionCards.get(key);
    if (existing) {
      // 修复原因：交互推进时 POST 新卡再 DELETE 旧卡会显示撤回痕迹。
      // callback token 更新失败后的降级路径也只能 PATCH 原 message_id，保持单卡身份稳定。
      await adapter?.updateTransientInteractionCard?.(existing.bot, existing.handle, message);
      return;
    }
    const handle = await adapter?.createTransientInteractionCard?.(bot, message);
    if (!handle) {
      return;
    }
    transientInteractionCards.set(key, { bot, taskId, handle });
  }

  async function finalizeTransientInteractionCard(
    actor: BotActor,
    fallback: BotOutboundMessage,
  ): Promise<boolean> {
    const key = getActorContextKey(actor);
    const existing = transientInteractionCards.get(key);
    if (!existing) {
      return false;
    }
    const adapter = providers[existing.bot.provider];
    try {
      // 修复原因：交互完成后撤回卡片会让问答和计划从聊天历史消失，用户无法回看
      // 决策上下文。终态只更新为无控件卡片并释放运行时句柄，后续交互会创建新卡。
      await adapter?.updateTransientInteractionCard?.(existing.bot, existing.handle, fallback);
    } catch (error) {
      botsLogger.warn(
        undefined,
        `finalize interaction card failed bot=${existing.bot.id} task=${existing.taskId}: ${error instanceof Error ? error.message : String(error)}`,
      );
    } finally {
      transientInteractionCards.delete(key);
    }
    return true;
  }

  /** per-bot 出站串行队列：同 bot FIFO + 最小间隔，不同 bot 互不阻塞。 */
  const botSendQueue = createBotSendQueue(BOT_SEND_MIN_INTERVAL_MS);

  /** 按出站消息的合并 id（chatId ?? userId）在该 bot 的对话表里定位会话槽位。 */
  async function readOutboundConversation(
    bot: BotConfig,
    message: BotOutboundMessage,
  ): Promise<{ key: string; state: BotContextState } | null> {
    const state = await repo.readState();
    const channel = state.bots[bot.id];
    if (!channel) {
      return null;
    }
    const entries = Object.entries(channel.conversations).filter(
      ([, conversation]) => conversation.conversationId === message.providerUserId,
    );
    const preferred =
      entries.find(([, conversation]) => conversation.conversationKind === "private") ??
      entries[0];
    return preferred ? { key: preferred[0], state: preferred[1] } : null;
  }

  /** 任一投递成功后清除一次性提示标记，使下一个失败 episode 能再次提示。 */
  async function clearDeliveryNoticeIfSet(
    bot: BotConfig,
    message: BotOutboundMessage,
  ): Promise<void> {
    const found = await readOutboundConversation(bot, message);
    if (!found?.state.deliveryNoticeAt) {
      return;
    }
    // 锁内原子 RMW（docs/specs/bot-state-ownership.md 规则 1）。
    await repo.mutateState((state) => {
      const channel = state.bots[bot.id];
      const conversation = channel?.conversations[found.key];
      if (!channel || !conversation?.deliveryNoticeAt) {
        return;
      }
      channel.conversations[found.key] = {
        ...conversation,
        deliveryNoticeAt: undefined,
        updatedAt: Date.now(),
      };
      channel.updatedAt = Date.now();
    });
  }

  /** 把无法投递的回复停放进对话槽位，并保证每个失败 episode 只提示一次。 */
  async function parkOutboundMessage(
    bot: BotConfig,
    message: BotOutboundMessage,
    code: BotSendFailureCode,
  ): Promise<void> {
    const found = await readOutboundConversation(bot, message);
    if (!found) {
      botsLogger.warn(
        undefined,
        `bot reply parked skipped: conversation not found bot=${bot.id} user=${message.providerUserId} code=${code}`,
      );
      return;
    }
    // 锁内原子 RMW；日志与一次性提示属于副作用，在锁外执行
    // （docs/specs/bot-state-ownership.md 规则 1/3）。
    let result: ReturnType<typeof enqueuePendingDelivery> | undefined;
    let shouldNotify = false;
    await repo.mutateState((state) => {
      const channel = state.bots[bot.id];
      const conversation = channel?.conversations[found.key];
      if (!channel || !conversation) {
        return;
      }
      const enqueued = enqueuePendingDelivery(
        conversation.pendingDeliveryQueue,
        { text: message.text, providerUserId: message.providerUserId },
        Date.now(),
      );
      result = enqueued;
      shouldNotify = !conversation.deliveryNoticeAt;
      channel.conversations[found.key] = {
        ...conversation,
        pendingDeliveryQueue: enqueued.queue,
        ...(shouldNotify ? { deliveryNoticeAt: Date.now() } : {}),
        updatedAt: Date.now(),
      };
      channel.updatedAt = Date.now();
    });
    if (!result) {
      return;
    }
    if (result.dropped) {
      botsLogger.warn(
        undefined,
        `bot reply parked queue full, oldest dropped bot=${bot.id} user=${message.providerUserId}`,
      );
    }
    botsLogger.warn(
      undefined,
      `bot reply parked bot=${bot.id} provider=${bot.provider} user=${message.providerUserId} code=${code}: ${message.text.slice(0, 80)}`,
    );
    if (shouldNotify) {
      // 提示本身发送失败只记日志：不再递归入队（spec 规则 5）。
      const adapter = providers[bot.provider];
      try {
        await adapter?.send(bot, {
          botId: bot.id,
          provider: bot.provider,
          providerUserId: message.providerUserId,
          text: msg(await readMessageLocale(), "deliveryParked"),
        });
      } catch (error) {
        botsLogger.warn(
          undefined,
          `delivery parked notice failed bot=${bot.id}: ${error instanceof Error ? error.message : String(error)}`,
        );
      }
    }
  }

  /**
   * 单条回复的发送 + 恢复：结构化错误按计划重试，恢复用尽后停放。
   * park:false 用于补投路径——条目已在队列里，失败由调用方递增 attempts。
   * 2026-10-10 事故：微信 context 过期后 /sendmessage 连续失败、回复被静默丢弃
   * （docs/specs/bot-outbound-delivery.md）。
   */
  async function deliverWithRecovery(
    bot: BotConfig,
    message: BotOutboundMessage,
    options: { park?: boolean } = {},
  ): Promise<"sent" | "parked"> {
    const adapter = providers[bot.provider];
    if (!adapter) {
      return "sent";
    }
    let current = message;
    let attempt = 0;
    for (;;) {
      try {
        await adapter.send(bot, current);
        await clearDeliveryNoticeIfSet(bot, current);
        return "sent";
      } catch (error) {
        const code = classifySendError(error);
        if (!code) {
          // 未分类失败保持既有语义：原样冒泡给调用方（stream 队列 warn / 回调错误回复）。
          throw error;
        }
        const detail =
          error instanceof BotSendError && error.detail ? ` detail=${error.detail}` : "";
        botsLogger.warn(
          undefined,
          `bot send failed bot=${bot.id} provider=${bot.provider} code=${code} attempt=${attempt}: ${error instanceof Error ? error.message : String(error)}${detail}`,
        );
        const freshToken = (await readOutboundConversation(bot, current))?.state.lastContextToken;
        const plan = planDeliveryRecovery(code, {
          attempt,
          usedToken: current.providerContextToken,
          freshToken,
        });
        if (plan.kind === "retry_with_token") {
          current = { ...current, providerContextToken: plan.token };
          attempt += 1;
          continue;
        }
        if (plan.kind === "retry_without_token") {
          const { providerContextToken: _expired, ...rest } = current;
          current = rest;
          attempt += 1;
          continue;
        }
        if (plan.kind === "retry_backoff") {
          await new Promise((resolve) => setTimeout(resolve, plan.delayMs));
          attempt += 1;
          continue;
        }
        if (options.park === false) {
          return "parked";
        }
        await parkOutboundMessage(bot, current, code);
        return "parked";
      }
    }
  }

  /**
   * 补投：按序投递停放回复；第一次失败即停止本轮（服务端状态对后续条目同样不利），
   * 失败条目 attempts+1，超限由队列纯函数丢弃并记 warn（spec 规则 6）。
   */
  async function flushPendingDeliveries(
    bot: BotConfig,
    context: BotContextState,
  ): Promise<BotContextState> {
    let current = context;
    for (;;) {
      const queue = current.pendingDeliveryQueue ?? [];
      const head = queue[0];
      if (!head) {
        return current;
      }
      const deliveryMessage: BotOutboundMessage = {
        botId: bot.id,
        provider: bot.provider,
        providerUserId: head.providerUserId,
        text: head.text,
        ...(current.lastContextToken ? { providerContextToken: current.lastContextToken } : {}),
      };
      let status: "sent" | "parked";
      try {
        status = await botSendQueue.run(bot.id, () =>
          deliverWithRecovery(bot, deliveryMessage, { park: false }),
        );
      } catch (error) {
        status = "parked";
        botsLogger.warn(
          undefined,
          `pending delivery flush error bot=${bot.id}: ${error instanceof Error ? error.message : String(error)}`,
        );
      }
      if (status === "sent") {
        current = { ...current, pendingDeliveryQueue: removeFirstPendingDelivery(queue).queue };
        await writeContext(current);
        continue;
      }
      const bumped = bumpFirstPendingDeliveryAttempt(queue);
      if (bumped.dropped) {
        botsLogger.warn(
          undefined,
          `pending delivery dropped after max attempts bot=${bot.id} user=${bumped.dropped.providerUserId}`,
        );
      }
      current = { ...current, pendingDeliveryQueue: bumped.queue };
      await writeContext(current);
      return current;
    }
  }

  /** 入站刷新：写入新鲜 context_token，随后用它补投停放的回复（spec 规则 6）。 */
  async function refreshDeliveryInbound(
    bot: BotConfig,
    context: BotContextState,
    providerContextToken: string,
  ): Promise<BotContextState> {
    let current = context;
    if (current.lastContextToken !== providerContextToken) {
      current = { ...current, lastContextToken: providerContextToken };
      await writeContext(current);
    }
    if (current.pendingDeliveryQueue?.length) {
      current = await flushPendingDeliveries(bot, current);
    }
    return current;
  }

  async function sendOutbound(bot: BotConfig, message: BotOutboundMessage): Promise<void> {
    const adapter = providers[bot.provider];
    if (!adapter) {
      return;
    }
    // 修复原因：微信出站在会话上下文过期后连续失败且回复被静默丢弃
    // （2026-10-10 事故）。统一走 per-bot 串行队列 + 结构化恢复 + 停放补投。
    await botSendQueue.run(bot.id, () => deliverWithRecovery(bot, message));
  }

  function buildInboundDeliveryKey(message: BotInboundMessage): string | null {
    const providerMessageId = message.actor.providerMessageId?.trim();
    if (!providerMessageId) {
      return null;
    }
    return [
      message.actor.botId,
      message.actor.provider,
      message.actor.chatId ?? message.actor.providerUserId,
      providerMessageId,
    ].join("::");
  }

  function releaseInboundDelivery(message: BotInboundMessage): void {
    const deliveryKey = buildInboundDeliveryKey(message);
    if (deliveryKey) {
      recentInboundDeliveryAtByKey.delete(deliveryKey);
    }
  }

  async function enqueueInboundProcessing<T>(actor: BotActor, task: () => Promise<T>): Promise<T> {
    const actorContextKey = getActorContextKey(actor);
    const previous = inboundProcessingQueuesByContext.get(actorContextKey) ?? Promise.resolve();
    let releaseQueue = (): void => undefined;
    const current = previous
      .catch(() => undefined)
      .then(
        () =>
          new Promise<void>((resolve) => {
            releaseQueue = resolve;
          }),
      );
    inboundProcessingQueuesByContext.set(actorContextKey, current);
    await previous.catch(() => undefined);
    try {
      // Bugfix: 同一个用户可能连续点击 AskUserQuestion 按钮或快速回复多条消息。
      // 这里按 actor 串行化入站处理，避免两个并发请求同时读取同一个 pendingElicitation 并重复 respondElicitation。
      return await task();
    } finally {
      releaseQueue();
      if (inboundProcessingQueuesByContext.get(actorContextKey) === current) {
        inboundProcessingQueuesByContext.delete(actorContextKey);
      }
    }
  }

  function pruneRecentInboundDeliveryDedupe(now: number): void {
    for (const [key, at] of recentInboundDeliveryAtByKey) {
      if (now - at >= BOT_INBOUND_DELIVERY_DEDUPE_TTL_MS) {
        recentInboundDeliveryAtByKey.delete(key);
      }
    }
  }

  function markInboundDelivery(message: BotInboundMessage): boolean {
    const now = Date.now();
    pruneRecentInboundDeliveryDedupe(now);
    const deliveryKey = buildInboundDeliveryKey(message);
    if (!deliveryKey) {
      return true;
    }
    if (recentInboundDeliveryAtByKey.has(deliveryKey)) {
      return false;
    }
    // Bugfix: 飞书 WebSocket 可能重投同一条 im.message.receive_v1，微信/Telegram 也可能在重试后重放同一 message id。
    // 普通消息有创建/发送任务的副作用，必须在进入业务处理前按 provider message id 幂等，避免同一句 hello 被执行两轮。
    recentInboundDeliveryAtByKey.set(deliveryKey, now);
    return true;
  }

  async function sendTyping(bot: BotConfig, actor: BotActor): Promise<void> {
    const adapter = providers[bot.provider];
    const targetId = actor.chatId ?? actor.providerUserId;
    if (!adapter?.sendTyping || !targetId) {
      return;
    }
    await adapter
      .sendTyping(bot, {
        providerUserId: targetId,
        providerMessageId: actor.providerMessageId,
        providerContextToken: actor.providerContextToken,
      })
      .catch(() => undefined);
  }

  async function stopInboundTyping(bot: BotConfig, actor: BotActor): Promise<void> {
    const adapter = providers[bot.provider];
    const targetId = actor.chatId ?? actor.providerUserId;
    if (!adapter?.stopTyping || !targetId || !actor.providerMessageId) {
      return;
    }
    const isLongRunningTyping = Array.from(typingTargets.values()).some(
      (typing) =>
        typing.bot.id === bot.id && typing.target.providerMessageId === actor.providerMessageId,
    );
    if (isLongRunningTyping) {
      return;
    }
    // Bugfix: 飞书 sendTyping 只负责给本次入站消息加 Typing reaction。
    // 短命令回复发送完成后必须按同一 messageId 显式删除，避免依赖定时兜底或等下一条命令清理。
    await adapter
      .stopTyping(bot, {
        providerUserId: targetId,
        providerMessageId: actor.providerMessageId,
        providerContextToken: actor.providerContextToken,
      })
      .catch(() => undefined);
  }

  function startTyping(bot: BotConfig, actor: BotActor, taskId: string): void {
    const adapter = providers[bot.provider];
    const targetId = actor.chatId ?? actor.providerUserId;
    // key 带 botId：一个会话可以绑定多个机器人，各渠道各自显示"正在输入"。
    const typingKey = buildTypingKey(taskId, bot.id);
    if (!adapter || !targetId || typingTargets.has(typingKey) || typingIntervals.has(typingKey)) {
      return;
    }
    const target: BotTypingTarget = {
      providerUserId: targetId,
      providerMessageId: actor.providerMessageId,
      providerContextToken: actor.providerContextToken,
    };
    if (adapter.startTyping) {
      typingTargets.set(typingKey, { bot, target });
      void adapter.startTyping(bot, target).catch(() => undefined);
      return;
    }
    if (adapter.sendTyping) {
      void adapter.sendTyping(bot, target).catch(() => undefined);
      typingIntervals.set(
        typingKey,
        setInterval(() => {
          void adapter.sendTyping?.(bot, target).catch(() => undefined);
        }, BOT_TYPING_INTERVAL_MS),
      );
    }
  }

  function stopTyping(taskId: string, botId: string): void {
    const typingKey = buildTypingKey(taskId, botId);
    const activeTyping = typingTargets.get(typingKey);
    if (activeTyping) {
      typingTargets.delete(typingKey);
      const adapter = providers[activeTyping.bot.provider];
      void adapter?.stopTyping?.(activeTyping.bot, activeTyping.target).catch(() => undefined);
    }
    const intervalId = typingIntervals.get(typingKey);
    if (!intervalId) {
      return;
    }
    clearInterval(intervalId);
    typingIntervals.delete(typingKey);
  }

  function updateLiveStatusProgress(event: ModeStreamEvent): void {
    if (event.type === "agent_message_chunk" || event.type === "agent_thought_chunk") {
      const text = normalizeStatusProgressText(event.content);
      if (!text) {
        return;
      }
      const kind = event.type === "agent_message_chunk" ? "message" : "thought";
      const previous = liveStatusProgressByTaskId.get(event.taskId);
      liveStatusProgressByTaskId.set(event.taskId, {
        kind,
        text: truncateLiveStatusProgressText(
          previous?.kind === kind ? `${previous.text}${text}` : text,
        ),
      });
      return;
    }
    if (event.type === "tool_call" || event.type === "tool_call_update") {
      const text = formatStatusStreamToolProgress(event);
      if (text) {
        liveStatusProgressByTaskId.set(event.taskId, { kind: "tool", text });
      }
    }
  }

  async function broadcastTaskListChange(
    context: Pick<BotContextState, "workspacePath" | "workspaceIdentity">,
    taskId: string,
    event: BotTaskBroadcastPayload["event"],
    extras: Partial<
      Pick<
        BotTaskBroadcastPayload,
        | "task"
        | "provider"
        | "configOptions"
        | "prompt"
        | "permissionRequest"
        | "elicitationRequest"
        | "requestId"
        | "error"
        | "source"
      >
    > = {},
  ): Promise<void> {
    await deps.broadcastService
      ?.send({
        channel: BOT_TASK_BROADCAST_CHANNEL,
        payload: {
          workspacePath: context.workspacePath,
          workspaceIdentity: context.workspaceIdentity,
          taskId,
          event,
          updatedAt: Date.now(),
          ...extras,
        },
      })
      .catch(() => undefined);
  }

  async function broadcastTaskStreamEvent(
    context: Pick<BotContextState, "workspacePath" | "workspaceIdentity">,
    event: ModeStreamEvent,
  ): Promise<void> {
    const payload: BotTaskStreamBroadcastPayload = {
      workspacePath: context.workspacePath,
      workspaceIdentity: context.workspaceIdentity,
      taskId: event.taskId,
      event,
      updatedAt: Date.now(),
    };
    await deps.broadcastService
      ?.send({
        channel: BOT_TASK_STREAM_BROADCAST_CHANNEL,
        payload,
      })
      .catch(() => undefined);
  }

  async function listWorkspaceRefs(
    params: BotListWorkspaceRefsParams = {},
  ): Promise<BotWorkspaceRef[]> {
    const cacheKey = params.currentWorkspace
      ? getWorkspaceKey(
          params.currentWorkspace.workspacePath,
          params.currentWorkspace.workspaceIdentity,
        )
      : "__default__";
    const nowMs = Date.now();
    const cached = cachedWorkspaceRefsByKey.get(cacheKey);
    if (cached && cached.expiresAt > nowMs) {
      return cached.value;
    }
    const workspaceByKey = new Map<string, BotWorkspaceRef>();
    if (params.currentWorkspace) {
      workspaceByKey.set(
        getWorkspaceKey(
          params.currentWorkspace.workspacePath,
          params.currentWorkspace.workspaceIdentity,
        ),
        params.currentWorkspace,
      );
    }

    const settings = await deps.settingService?.get().catch(() => null);
    for (const entry of settings?.lastWorkspaceSession ?? []) {
      const workspace = createWorkspaceRef(
        entry.workspacePath,
        entry.kind === "remote" ? entry.workspaceIdentity : undefined,
      );
      workspaceByKey.set(
        getWorkspaceKey(workspace.workspacePath, workspace.workspaceIdentity),
        workspace,
      );
    }

    const value = [...workspaceByKey.values()];
    cachedWorkspaceRefsByKey.set(cacheKey, {
      expiresAt: nowMs + BOT_WORKSPACE_REFS_CACHE_TTL_MS,
      value,
    });
    return value;
  }

  function resolveCanonicalContextWorkspace(
    context: Pick<BotContextState, "workspacePath" | "workspaceIdentity" | "workspaceId">,
    workspaces: readonly BotWorkspaceRef[],
  ): BotWorkspaceRef | null {
    const currentWorkspaceKey = getWorkspaceKey(context.workspacePath, context.workspaceIdentity);
    const exactWorkspace = workspaces.find(
      (workspace) =>
        getWorkspaceKey(workspace.workspacePath, workspace.workspaceIdentity) ===
        currentWorkspaceKey,
    );
    if (exactWorkspace) {
      return exactWorkspace;
    }
    if (context.workspaceId) {
      const workspaceById = workspaces.find((workspace) => workspace.id === context.workspaceId);
      if (workspaceById) {
        return workspaceById;
      }
    }
    // Bugfix: path-only 旧状态只能靠 path 候选回填 remote identity。
    // 这里只在同 path 候选唯一时才升级，避免把两个不同 remote workspace 错绑到同一身份。
    // 已经带 workspaceIdentity 的远端 context 不能被同路径本地候选降级，否则 /workspace 会丢失远端项。
    if (context.workspaceIdentity) {
      return null;
    }
    const samePathWorkspaces = workspaces.filter(
      (workspace) => workspace.workspacePath === context.workspacePath,
    );
    return samePathWorkspaces.length === 1 ? samePathWorkspaces[0]! : null;
  }

  async function normalizeBotWorkspaceConfig(
    config: BotsConfigFile,
    bot: BotConfig,
    currentWorkspace?: BotWorkspaceRef,
  ): Promise<{
    config: BotsConfigFile;
    bot: BotConfig;
    user: BotConfig;
    workspaces: BotWorkspaceRef[];
  }> {
    const workspaces = await listWorkspaceRefs({ currentWorkspace });
    const nextAllowedWorkspaces = normalizeConfiguredAllowedWorkspaces(
      bot.allowedWorkspaces,
      workspaces,
    );
    const nextBot: BotConfig = {
      ...bot,
      // Bugfix: workspace 候选项现在来自 settings.lastWorkspaceSession，不再写入 bot-config.json。
      // 这里顺手把旧的 path-only workspace 授权升级成 workspaceIdentity key，避免 remote context 自愈后
      // 授权侧还停留在旧路径语义，导致消息链路被误判成 workspaceOutOfScope。
      allowedWorkspaces: nextAllowedWorkspaces,
    };
    const nextConfig: BotsConfigFile = {
      ...config,
      bots: config.bots.map((item) => (item.id === bot.id ? nextBot : item)),
    };
    const shouldWrite = nextBot.allowedWorkspaces.join("\n") !== bot.allowedWorkspaces.join("\n");
    if (shouldWrite) {
      await repo.writeConfig(nextConfig);
    }
    return { config: nextConfig, bot: nextBot, user: nextBot, workspaces };
  }

  async function readTaskMeta(
    context: Pick<BotContextState, "workspacePath" | "workspaceIdentity">,
    taskId: string,
  ): Promise<ModeTaskMeta | null> {
    const modeTaskService = await resolveModeTaskServiceForContext(context);
    const tasks = await modeTaskService.listTasks({
      workspacePath: context.workspacePath,
      workspaceIdentity: context.workspaceIdentity,
    });
    return tasks.find((task) => task.taskId === taskId) ?? null;
  }

  async function listContextTaskSelectionEntries(
    context: BotContextState,
    user: BotConfig,
  ): Promise<BotTaskSelectionEntry[]> {
    const currentWorkspace = createWorkspaceRef(context.workspacePath, context.workspaceIdentity);
    const currentWorkspaceKey = getWorkspaceKey(context.workspacePath, context.workspaceIdentity);
    const workspaceRefs = await listWorkspaceRefs({ currentWorkspace });
    const allowedWorkspaces = filterAllowedWorkspaces(workspaceRefs, user.allowedWorkspaces);
    const candidateWorkspaces = allowedWorkspaces.filter((workspace) => {
      const workspaceKey = getWorkspaceKey(workspace.workspacePath, workspace.workspaceIdentity);
      return (
        workspaceKey === currentWorkspaceKey ||
        (!context.workspaceIdentity && workspace.workspacePath === context.workspacePath)
      );
    });
    const workspaces = candidateWorkspaces.length > 0 ? candidateWorkspaces : [currentWorkspace];
    const entries = (
      await Promise.all(
        workspaces.map(async (workspace) => {
          const modeTaskService = await resolveModeTaskServiceForContext(workspace);
          const tasks = await modeTaskService
            .listTasks({
              workspacePath: workspace.workspacePath,
              workspaceIdentity: workspace.workspaceIdentity,
            })
            .catch(() => []);
          return tasks.map((task) => ({
            task,
            workspacePath: workspace.workspacePath,
            workspaceIdentity: workspace.workspaceIdentity,
          }));
        }),
      )
    ).flat();
    const entryByKey = new Map<string, BotTaskSelectionEntry>();
    for (const entry of entries) {
      entryByKey.set(
        `${getWorkspaceKey(entry.workspacePath, entry.workspaceIdentity)}:${entry.task.taskId}`,
        entry,
      );
    }
    return [...entryByKey.values()];
  }

  function resolvePendingTaskSelectionEntry(
    actor: BotActor,
    value: string,
  ): BotTaskSelectionEntry | null {
    const actorContextKey = getActorContextKey(actor);
    const directEntry = pendingTaskSelectionsByContext.get(actorContextKey)?.get(value.trim());
    if (directEntry) {
      return directEntry;
    }
    const option = resolvePendingSelectionOption(actor, "task.set", value);
    if (!option) {
      return null;
    }
    return pendingTaskSelectionsByContext.get(actorContextKey)?.get(option.id) ?? null;
  }

  function resolvePendingWorkspaceSelectionEntry(
    actor: BotActor,
    value: string,
  ): BotWorkspaceSelectionEntry | null {
    const actorContextKey = getActorContextKey(actor);
    const directEntry = pendingWorkspaceSelectionsByContext.get(actorContextKey)?.get(value.trim());
    if (directEntry) {
      return directEntry;
    }
    const option = resolvePendingSelectionOption(actor, "workspace.set", value);
    if (!option) {
      return null;
    }
    return pendingWorkspaceSelectionsByContext.get(actorContextKey)?.get(option.id) ?? null;
  }

  function createCurrentWorkspaceRef(context: BotContextState): BotWorkspaceRef {
    return createWorkspaceRef(context.workspacePath, context.workspaceIdentity);
  }

  async function readContextActiveTaskMeta(
    context: BotContextState,
  ): Promise<ModeTaskMeta | null> {
    if (!context.activeTaskId) {
      return null;
    }
    const listedTask = await readTaskMeta(context, context.activeTaskId).catch(() => null);
    if (listedTask) {
      return listedTask;
    }
    return (
      (
        await (
          await resolveModeTaskServiceForContext(context)
        )
          .getTaskSnapshot({
            taskId: context.activeTaskId,
            workspacePath: context.workspacePath,
            workspaceIdentity: context.workspaceIdentity,
          })
          .catch(() => null)
      )?.meta ?? null
    );
  }

  async function requireActiveTask(
    message: BotInboundMessage,
    auth: {
      context: BotContextState;
      locale: Locale | undefined;
    },
  ): Promise<
    | {
        ok: true;
        taskId: string;
        task: ModeTaskMeta;
        configOptions: ModeConfigOption[];
      }
    | { ok: false; reply: BotOutboundMessage[] }
  > {
    if (!auth.context.activeTaskId) {
      return {
        ok: false,
        reply: [createOutbound(message.actor, msg(auth.locale, "noActiveTask"))],
      };
    }
    const task = await readContextActiveTaskMeta(auth.context);
    if (!task) {
      return {
        ok: false,
        reply: [createOutbound(message.actor, msg(auth.locale, "noActiveTask"))],
      };
    }
    const configOptions = await listActiveTaskConfigOptions(
      auth.context,
      auth.context.activeTaskId,
    );
    return {
      ok: true,
      taskId: auth.context.activeTaskId,
      task,
      configOptions,
    };
  }

  async function broadcastTaskConfigSync(params: {
    context: BotContextState;
    taskId: string;
    task?: ModeTaskMeta | null;
    provider?: ModeProvider;
    configOptions?: ModeConfigOption[];
  }): Promise<void> {
    await broadcastTaskListChange(params.context, params.taskId, "updated", {
      ...(params.task ? { task: params.task } : {}),
      ...(params.provider ? { provider: params.provider } : {}),
      ...(params.configOptions ? { configOptions: params.configOptions } : {}),
    });
  }

  function isTerminalTaskMeta(
    task: ModeTaskMeta | null,
    eventType: "task_complete" | "task_error",
  ): boolean {
    if (!task) {
      return false;
    }
    if (eventType === "task_error") {
      return task.status === "error";
    }
    return task.status === "completed";
  }

  async function readTerminalTaskMeta(
    context: Pick<BotContextState, "workspacePath" | "workspaceIdentity">,
    taskId: string,
    eventType: "task_complete" | "task_error",
  ): Promise<ModeTaskMeta | null> {
    let latestTask = await readTaskMeta(context, taskId).catch(() => null);
    if (isTerminalTaskMeta(latestTask, eventType)) {
      return latestTask;
    }

    for (const retryDelayMs of BOT_TASK_META_RETRY_DELAYS_MS) {
      await delay(retryDelayMs);
      latestTask = await readTaskMeta(context, taskId).catch(() => latestTask);
      if (isTerminalTaskMeta(latestTask, eventType)) {
        return latestTask;
      }
    }

    return latestTask;
  }

  async function processProviderCallback(
    provider: BotProvider,
    payload: unknown,
  ): Promise<BotProviderCallbackResult> {
    const adapter = providers[provider];
    if (!adapter) {
      return { ok: false, replies: [], status: 400 };
    }
    const locale = await readMessageLocale();
    const config = await repo.readConfig();
    const callbackBot = findCallbackBot(config, provider, payload);
    const preparedPayload = callbackBot
      ? ((await adapter.prepareCallbackPayload?.(callbackBot, payload).catch((error: unknown) => ({
          modeCallbackPrepareError: error instanceof Error ? error.message : String(error),
        }))) ?? payload)
      : payload;
    if (
      isRecord(preparedPayload) &&
      typeof preparedPayload.modeCallbackPrepareError === "string"
    ) {
      return {
        ok: false,
        replies: [],
        responseBody: { error: preparedPayload.modeCallbackPrepareError },
        status: 401,
      };
    }
    const callbackResponse = callbackBot
      ? await adapter.handleCallbackResponse?.(callbackBot, preparedPayload)
      : null;
    if (callbackResponse?.responseBody !== undefined) {
      return {
        ok: (callbackResponse.status ?? 200) < 400,
        replies: [],
        responseBody: callbackResponse.responseBody,
        status: callbackResponse.status,
      };
    }
    const parsePayload =
      isFeishuBotProvider(provider) && isRecord(preparedPayload)
        ? { modeProvider: provider, ...preparedPayload }
        : preparedPayload;
    const parsedInboundMessages = adapter.parseCallback(parsePayload);
    if (isFeishuBotProvider(provider)) {
      // Bugfix: 飞书 WebSocket connected 只代表长连接已建成，不代表事件订阅已经推到本机。
      // 这里记录入口 payload 摘要和解析数量，方便区分“飞书未推事件”和“payload 形状未被解析”。
      botsLogger.debug(
        undefined,
        `provider callback parsed provider=${provider} count=${parsedInboundMessages.length} ${summarizeCallbackPayload(preparedPayload)}`,
      );
    }
    const replies: BotOutboundMessage[] = [];
    let hadBusinessFailure = false;
    const inboundSecret =
      isRecord(preparedPayload) && typeof preparedPayload.webhookSecret === "string"
        ? preparedPayload.webhookSecret
        : undefined;
    for (const inbound of parsedInboundMessages) {
      const bot = findBot(config, inbound.botId);
      if (bot?.provider === "webhook" && bot.webhookSecretRef) {
        const expectedSecret = await deps.credentialService.load(bot.webhookSecretRef);
        if (expectedSecret && expectedSecret !== inboundSecret) {
          replies.push(createOutbound(inbound.actor, msg(locale, "webhookSecretInvalid")));
          continue;
        }
      }
      if (bot && isFeishuBotProvider(bot.provider) && bot.webhookSecretRef) {
        const expectedToken = await deps.credentialService.load(bot.webhookSecretRef);
        const payloadHeader =
          isRecord(preparedPayload) && isRecord(preparedPayload.header)
            ? preparedPayload.header
            : null;
        const inboundToken =
          isRecord(preparedPayload) && typeof preparedPayload.token === "string"
            ? preparedPayload.token
            : typeof payloadHeader?.token === "string"
              ? payloadHeader.token
              : undefined;
        if (expectedToken && expectedToken !== inboundToken) {
          replies.push(createOutbound(inbound.actor, msg(locale, "webhookSecretInvalid")));
          continue;
        }
      }
      let inboundMessage = inbound;
      if (bot && !inbound.actor.displayName && adapter.resolveActorDisplayName) {
        try {
          const displayName = await adapter.resolveActorDisplayName(bot, inbound.actor);
          if (displayName?.trim()) {
            inboundMessage = {
              ...inbound,
              actor: {
                ...inbound.actor,
                displayName: displayName.trim(),
              },
            };
          }
        } catch (error) {
          // Bugfix: 飞书 displayName 需要额外通讯录权限，权限缺失时不能阻断消息处理和绑定。
          botsLogger.debug(
            undefined,
            `resolve actor displayName failed provider=${provider} bot=${inbound.botId} user=${inbound.actor.providerUserId}: ${error instanceof Error ? error.message : String(error)}`,
          );
        }
      }
      if (!markInboundDelivery(inboundMessage)) {
        botsLogger.info(
          undefined,
          `provider callback duplicated provider=${provider} bot=${inboundMessage.botId} user=${inboundMessage.actor.providerUserId} messageId=${inboundMessage.actor.providerMessageId ?? ""}`,
        );
        continue;
      }
      botsLogger.info(
        undefined,
        `provider callback provider=${provider} bot=${inboundMessage.botId} user=${inboundMessage.actor.providerUserId} displayName=${inboundMessage.actor.displayName ?? ""} text=${inboundMessage.text}`,
      );
      let outbound: BotOutboundMessage[];
      let reconnectStartingReply: BotOutboundMessage | null = null;
      let inboundBusinessFailure = false;
      try {
        const command = parseBotCommand(inboundMessage.text);
        if (bot && command.type === "reconnect") {
          outbound = await handleReconnect(inboundMessage, {
            onReconnectStart: async (auth) => {
              reconnectStartingReply = createOutbound(
                inboundMessage.actor,
                msg(auth.locale, "remoteReconnectStarting", {
                  workspacePath: auth.context.workspacePath,
                }),
              );
              await sendOutbound(bot, reconnectStartingReply);
            },
          });
        } else {
          outbound = await service.handleInboundMessage(inboundMessage);
        }
      } catch (error) {
        releaseInboundDelivery(inboundMessage);
        hadBusinessFailure = true;
        inboundBusinessFailure = true;
        const message = error instanceof Error ? error.message : String(error);
        const userFacingMessage = formatUserFacingBotError(error, locale);
        botsLogger.warn(
          undefined,
          `provider callback failed provider=${provider} bot=${inboundMessage.botId} user=${inboundMessage.actor.providerUserId}: ${message}`,
        );
        outbound = [
          createOutbound(
            inboundMessage.actor,
            isSessionExpiredError(error)
              ? userFacingMessage
              : msg(locale, "callbackFailed", { message: userFacingMessage }),
          ),
        ];
      }
      if (reconnectStartingReply) {
        replies.push(reconnectStartingReply);
      }
      replies.push(...outbound);
      if (inboundBusinessFailure) {
        // Bugfix：错误提示发送成功不等于业务消息已经消费成功。此处不能执行 callback ACK，
        // 否则飞书会移除按钮；最终 ok=false 也会阻止 Telegram/微信提交外部游标。
        if (bot) {
          for (const outboundMessage of outbound) {
            await sendOutbound(bot, outboundMessage).catch((sendError) => {
              botsLogger.warn(
                undefined,
                `provider callback failure notice failed provider=${provider} bot=${bot.id}: ${sendError instanceof Error ? sendError.message : String(sendError)}`,
              );
            });
          }
          await stopInboundTyping(bot, inboundMessage.actor).catch(() => undefined);
        }
        continue;
      }
      if (bot) {
        const transientCard = transientInteractionCards.get(
          getActorContextKey(inboundMessage.actor),
        );
        const handledByFeishuSynchronousCardAction =
          isFeishuBotProvider(provider) &&
          isRecord(preparedPayload) &&
          preparedPayload.modeFeishuSynchronousCardAction === true &&
          Boolean(outbound[0]);
        // Bugfix: 只做空 ACK 会让 Telegram 顶部 loading 消失但没有任何可见反馈。
        // 这里在业务处理后把结果写进 answerCallbackQuery 的 toast，即使后续 sendMessage 失败，用户也能看到按钮结果。
        const callbackText = outbound[0]?.text ?? msg(locale, "received");
        let acknowledgeResult:
          | Awaited<ReturnType<NonNullable<typeof adapter.acknowledgeCallback>>>
          | undefined;
        const acknowledgeController = new AbortController();
        const acknowledgeTimeout = setTimeout(() => {
          acknowledgeController.abort(
            new Error(
              `Bot provider callback acknowledgement timed out after ${BOT_PROVIDER_CALLBACK_ACK_TIMEOUT_MS}ms.`,
            ),
          );
        }, BOT_PROVIDER_CALLBACK_ACK_TIMEOUT_MS);
        try {
          acknowledgeResult = await Promise.race([
            handledByFeishuSynchronousCardAction || (transientCard && !outbound[0]?.elicitation)
              ? Promise.resolve(undefined)
              : adapter.acknowledgeCallback?.(
                  bot,
                  preparedPayload,
                  callbackText,
                  outbound[0],
                  acknowledgeController.signal,
                ),
            new Promise<never>((_resolve, reject) => {
              acknowledgeController.signal.addEventListener(
                "abort",
                () => reject(acknowledgeController.signal.reason),
                { once: true },
              );
            }),
          ]);
        } catch (error) {
          // 修复原因：飞书第二题原本必须等待 card/update 完成；credential 或 SDK 内部
          // 任一步骤悬挂都会压住 fallback。主流程自己设 deadline，超时后立即另发下一题。
          botsLogger.warn(
            undefined,
            `provider callback acknowledge failed provider=${provider} bot=${bot.id}: ${error instanceof Error ? error.message : String(error)}`,
          );
        } finally {
          clearTimeout(acknowledgeTimeout);
        }
        const callbackHandledByCardUpdate =
          handledByFeishuSynchronousCardAction || acknowledgeResult?.handled === true;
        if (
          !handledByFeishuSynchronousCardAction &&
          callbackHandledByCardUpdate &&
          transientCard &&
          outbound[0]?.elicitation
        ) {
          // 修复原因：真实飞书日志确认 card/update 返回成功后客户端仍可能停在旧题。
          // callback token 负责点击 ACK，随后再 PATCH 同一 message_id 强制刷新可见结构；
          // 两次写入始终指向同一张卡，禁止退回“新建后撤回”的闪烁方案。
          await providers[transientCard.bot.provider]
            ?.updateTransientInteractionCard?.(transientCard.bot, transientCard.handle, outbound[0])
            .catch((error) => {
              // 业务回答已被 Agent 接受且 callback token 已完成 ACK，PATCH 失败不能让
              // Telegram/微信式外部游标重试整次回答，否则会重复提交同一交互。
              botsLogger.warn(
                undefined,
                `refresh transient interaction card failed provider=${provider} bot=${bot.id}: ${error instanceof Error ? error.message : String(error)}`,
              );
            });
        }
        if (
          callbackHandledByCardUpdate &&
          transientCard &&
          outbound[0]?.elicitation?.status !== "pending"
        ) {
          // 修复原因：card_update_token 已把同一消息更新为只读终态，此时只释放内存句柄，
          // 不能再 PATCH、DELETE 或另发结果卡。
          transientInteractionCards.delete(getActorContextKey(inboundMessage.actor));
        }
        // Bugfix: /reconnect 的“正在重连”必须在 ensureConnected 前实时发送。
        // handleReconnect 只返回最终结果，避免重连完成后才把过期的开始状态一起吐给用户。
        try {
          for (const outboundMessage of callbackHandledByCardUpdate ? [] : outbound) {
            if (transientCard) {
              if (outboundMessage.selection || outboundMessage.elicitation?.status === "pending") {
                await upsertTransientInteractionCard(
                  bot,
                  inboundMessage.actor,
                  transientCard.taskId,
                  outboundMessage,
                );
                continue;
              }
              if (
                outboundMessage.elicitation ||
                /^\/(?:approve|deny)(?:\s|$)/u.test(inboundMessage.text)
              ) {
                await finalizeTransientInteractionCard(inboundMessage.actor, outboundMessage);
                continue;
              }
            }
            await sendOutbound(bot, outboundMessage);
          }
          await stopInboundTyping(bot, inboundMessage.actor);
        } catch (error) {
          releaseInboundDelivery(inboundMessage);
          throw error;
        }
      }
    }
    return {
      ok: !hadBusinessFailure,
      replies,
      ...(hadBusinessFailure ? { status: 503 } : {}),
    };
  }

  function createAssistantReplyBlocks(
    parts: readonly ModeAssistantMessagePart[],
    toolCalls: ReadonlyMap<string, BotReplyToolCallState>,
    mode: BotReplyGranularity | undefined,
    changeSummary: ModeTaskMeta["changeSummary"] | null | undefined,
  ): BotAssistantReplyBlock[] {
    const blocks: BotAssistantReplyBlock[] = [];
    const resolvedMode = mode ?? getDefaultBotReplyGranularity();
    const presentation = buildModeAssistantPresentation({
      content: "",
      toolCalls: [...toolCalls.values()].map((toolCall) => ({
        ...toolCall,
        kind: toolCall.kind ?? "tool",
        input: toolCall.input,
        status: toolCall.status ?? "pending",
      })),
      parts,
    });

    if (resolvedMode === "summary_changes") {
      if (presentation.latestPart?.content.trim()) {
        blocks.push({
          type: "content",
          content: presentation.latestPart.content,
        });
      }
    } else {
      for (const block of presentation.blocks) {
        if (block.type === "content" && block.content.trim()) {
          blocks.push({ type: "content", content: block.content });
          continue;
        }
        if (resolvedMode !== "assistant_toolcalls_changes" || block.type !== "tool-call") {
          continue;
        }
        const toolCall = toolCalls.get(block.toolCall.toolId);
        if (toolCall) {
          blocks.push({ type: "tool-call", toolCall });
        }
      }
    }

    if (changeSummary && changeSummary.fileCount > 0 && changeSummary.files.length > 0) {
      blocks.push({ type: "change-summary", changeSummary });
    }
    return blocks;
  }

  function normalizeBotElicitationQuestions(
    event: Extract<ModeStreamEvent, { type: "elicitation_request" }>,
    locale: Locale | undefined,
  ): ModeElicitationQuestion[] {
    const schema = isRecord(event.schema) ? event.schema : null;
    const isPlanApproval = schema?.interaction === "plan_approval";
    if (isPlanApproval) {
      return [
        {
          question: msg(locale, "planApprovalTitle"),
          header: msg(locale, "planApprovalHeader"),
          options: [
            {
              value: "approve",
              label: msg(locale, "planApprovalApprove"),
              description: msg(locale, "planApprovalApproveDescription"),
            },
          ],
        },
      ];
    }
    const sourceQuestions =
      event.questions && event.questions.length > 0
        ? event.questions
        : [
            {
              question: event.message,
              header: event.header ?? event.message,
              options: event.options,
              ...(event.multiSelect ? { multiSelect: true } : {}),
            },
          ];
    return sourceQuestions.map((question) => ({
      question: question.question,
      header: question.header || question.question,
      options: question.options.map((option) => ({
        value: option.value,
        label: option.label || option.value,
        description: option.description,
      })),
      ...(question.multiSelect ? { multiSelect: true } : {}),
    }));
  }

  function readBotElicitationRenderContext(
    event: Extract<ModeStreamEvent, { type: "elicitation_request" }>,
  ): BotPendingElicitation["renderContext"] {
    const schema = isRecord(event.schema) ? event.schema : null;
    if (
      schema?.interaction !== "plan_approval" ||
      typeof schema.plan !== "string" ||
      !schema.plan.trim()
    ) {
      return undefined;
    }
    return { kind: "plan_approval", plan: schema.plan.trim() };
  }

  function createPendingElicitationSchema(
    pending: BotPendingElicitation,
  ): Record<string, unknown> | undefined {
    return pending.renderContext?.kind === "plan_approval"
      ? { interaction: "plan_approval", plan: pending.renderContext.plan }
      : undefined;
  }

  function getElicitationAnswerKey(questionIndex: number): string {
    return String(questionIndex);
  }

  function readElicitationAnswerValues(
    pending: BotPendingElicitation,
    questionIndex: number,
  ): string[] {
    return pending.answers[getElicitationAnswerKey(questionIndex)] ?? [];
  }

  function getPendingElicitationSelectionToken(pending: BotPendingElicitation): string {
    return createHash("sha256")
      .update(
        [pending.taskId, pending.runId, pending.requestId, pending.currentQuestionIndex].join("::"),
      )
      .digest("hex")
      .slice(0, 12);
  }

  function getPendingElicitationSkipOptionId(pending: BotPendingElicitation): string {
    return `${BOT_ELICITATION_SKIP_OPTION_ID}:${getPendingElicitationSelectionToken(pending)}`;
  }

  function parseElicitationResponseValue(value: string): {
    token?: string;
    value: string;
  } {
    const trimmed = value.trim();
    const [maybeToken, ...rest] = trimmed.split(/\s+/u);
    if (maybeToken && rest.length > 0 && /^[a-f0-9]{12}$/iu.test(maybeToken)) {
      return { token: maybeToken.toLowerCase(), value: rest.join(" ") };
    }
    return { value: trimmed };
  }

  function parseElicitationFormValues(value: string): string[] | null {
    if (!value.startsWith(BOT_ELICITATION_FORM_VALUE_PREFIX)) {
      return null;
    }
    const encoded = value.slice(BOT_ELICITATION_FORM_VALUE_PREFIX.length);
    try {
      const parsed = JSON.parse(decodeURIComponent(encoded)) as unknown;
      const values = Array.isArray(parsed) ? parsed : [parsed];
      return values.map((item) => (typeof item === "string" ? item.trim() : "")).filter(Boolean);
    } catch {
      return [];
    }
  }

  function mergeElicitationFormValues(
    question: ModeElicitationQuestion,
    selectedValues: readonly string[],
    formValues: readonly string[],
  ): string[] {
    const customValues = formValues.filter(Boolean);
    if (!question.multiSelect) {
      return customValues.length > 0 ? customValues.slice(0, 1) : selectedValues.slice(0, 1);
    }
    const merged: string[] = [];
    for (const value of [...selectedValues, ...customValues]) {
      if (!value || merged.includes(value)) {
        continue;
      }
      merged.push(value);
    }
    return merged;
  }

  function toggleElicitationCustomAnswerExpanded(
    pending: BotPendingElicitation,
  ): BotPendingElicitation {
    const current = new Set(pending.expandedCustomAnswerQuestionIndexes ?? []);
    if (current.has(pending.currentQuestionIndex)) {
      current.delete(pending.currentQuestionIndex);
    } else {
      current.add(pending.currentQuestionIndex);
    }
    return {
      ...pending,
      expandedCustomAnswerQuestionIndexes: [...current].sort((left, right) => left - right),
    };
  }

  function resolveElicitationQuestionValue(
    question: ModeElicitationQuestion,
    value: string,
    options: { includeSubmit?: boolean } = {},
  ): string {
    const trimmed = value.trim();
    if (!trimmed) {
      return "";
    }
    const normalized = normalizeText(trimmed);
    if (
      question.multiSelect &&
      options.includeSubmit !== false &&
      ["submit", "done", "完成", "提交", BOT_ELICITATION_SUBMIT_OPTION_ID].includes(normalized)
    ) {
      return BOT_ELICITATION_SUBMIT_OPTION_ID;
    }
    const option = resolveOptionByValue(
      question.options.map((item) => ({ id: item.value, label: item.label })),
      trimmed,
    );
    if (option) {
      return option.id;
    }
    const index = Number.parseInt(trimmed, 10);
    if (
      question.multiSelect &&
      options.includeSubmit !== false &&
      /^[1-9]\d*$/u.test(trimmed) &&
      index === question.options.length + 1
    ) {
      return BOT_ELICITATION_SUBMIT_OPTION_ID;
    }
    return trimmed;
  }

  function isPendingElicitationOwnedByActor(
    pending: BotPendingElicitation,
    actor: BotActor,
  ): boolean {
    return !pending.actorKey || pending.actorKey === getActorContextKey(actor);
  }

  function clearPendingElicitationSelection(pending: BotPendingElicitation): void {
    const token = getPendingElicitationSelectionToken(pending);
    for (const [contextKey, selection] of pendingSelectionsByContext) {
      if (
        selection.action === "elicitation.respond" &&
        (selection.token === token || selection.id.startsWith(`elicitation-${pending.requestId}-`))
      ) {
        pendingSelectionsByContext.delete(contextKey);
      }
    }
  }

  function buildBotElicitationContent(
    pending: BotPendingElicitation,
    answers: BotPendingElicitation["answers"] = pending.answers,
  ): Record<string, unknown> {
    // 修复原因：Bot 与桌面共用“缺少 key 表示跳过”的问答契约；未答题不能写成
    // 空字符串，否则 Agent 会把它误判为用户提供的偏好。
    const answerEntries = pending.questions.flatMap((question, index) => {
      const values = answers[getElicitationAnswerKey(index)] ?? [];
      const text = values.join(", ").trim();
      return text ? [[question.question, text]] : [];
    });
    const content: Record<string, unknown> = {
      answers: Object.fromEntries(answerEntries),
    };
    pending.questions.forEach((question, index) => {
      const values = answers[getElicitationAnswerKey(index)] ?? [];
      if (values.length > 0) {
        content[`answer_${index}`] = question.multiSelect ? values : values[0];
      }
    });
    if (pending.questions.length === 1) {
      const values = answers[getElicitationAnswerKey(0)] ?? [];
      if (values.length > 0) {
        content.answer = pending.questions[0]?.multiSelect ? values : values[0];
      }
    }
    return content;
  }

  function createBotElicitationRequestSnapshot(
    pending: BotPendingElicitation,
  ): ModeElicitationRequest {
    const currentQuestion = pending.questions[pending.currentQuestionIndex] ?? pending.questions[0];
    const answerDrafts = Object.fromEntries(
      Object.entries(pending.answers).map(([index, values]) => [`answer_${index}`, values]),
    );
    return {
      type: "elicitation_request",
      taskId: pending.taskId,
      traceId: pending.runId,
      requestId: pending.requestId,
      ...(pending.origin ? { origin: pending.origin } : {}),
      message: currentQuestion?.question ?? "",
      header: currentQuestion?.header,
      options: currentQuestion?.options ?? [],
      ...(currentQuestion?.multiSelect ? { multiSelect: true } : {}),
      questions: pending.questions,
      currentQuestionIndex: pending.currentQuestionIndex,
      answerDrafts,
      ...(createPendingElicitationSchema(pending)
        ? { schema: createPendingElicitationSchema(pending) }
        : {}),
    };
  }

  async function broadcastPendingElicitationProgress(
    context: BotContextState,
    pending: BotPendingElicitation,
  ): Promise<void> {
    let timeout: ReturnType<typeof setTimeout> | undefined;
    const outcome = await Promise.race([
      broadcastTaskListChange(context, pending.taskId, "elicitation_request", {
        elicitationRequest: createBotElicitationRequestSnapshot(pending),
        requestId: pending.requestId,
      }).then(() => "broadcast" as const),
      new Promise<"timeout">((resolve) => {
        timeout = setTimeout(
          () => resolve("timeout"),
          BOT_ELICITATION_PROGRESS_BROADCAST_TIMEOUT_MS,
        );
      }),
    ]);
    if (timeout) {
      clearTimeout(timeout);
    }
    if (outcome === "timeout") {
      // 修复原因：v4/UI 进度广播只是辅助同步。广播 RPC 悬挂时若一直 await，
      // 飞书按钮回调无法生成下一题，也到不了 card/update，用户会永久停在第一题。
      botsLogger.warn(
        undefined,
        `elicitation progress broadcast timed out task=${pending.taskId} request=${pending.requestId}`,
      );
    }
  }

  function formatBotElicitationTitle(
    pending: BotPendingElicitation,
    locale: Locale | undefined,
  ): string {
    const question = pending.questions[pending.currentQuestionIndex];
    if (!question) {
      return pending.requestId;
    }
    const isCustomAnswerExpanded =
      pending.expandedCustomAnswerQuestionIndexes?.includes(pending.currentQuestionIndex) === true;
    if (pending.renderContext?.kind === "plan_approval") {
      // Bugfix: Feishu 卡片能直接消费 schema.plan，但 Telegram/微信只渲染 message.text。
      // 在共享出站标题中投影完整计划，确保所有纯文本渠道都保留审批上下文。
      return [
        pending.renderContext.plan,
        "------",
        msg(locale, "planApprovalTitle"),
        isCustomAnswerExpanded ? msg(locale, "elicitationCustomPlaceholder") : null,
      ]
        .filter((part): part is string => typeof part === "string" && part.length > 0)
        .join("\n\n");
    }
    const parts = [
      pending.questions.length > 1
        ? `${pending.currentQuestionIndex + 1}/${pending.questions.length}`
        : null,
      question.header && question.header !== question.question ? question.header : null,
      question.question,
      question.multiSelect ? msg(locale, "elicitationMultiSelectHint") : null,
      isCustomAnswerExpanded ? msg(locale, "elicitationCustomPlaceholder") : null,
      msg(locale, "elicitationTextHint"),
    ].filter((part): part is string => typeof part === "string" && part.length > 0);
    return parts.join("\n");
  }

  function createBotElicitationSelection(
    pending: BotPendingElicitation,
    locale: Locale | undefined,
  ): SelectionPrompt {
    const question = pending.questions[pending.currentQuestionIndex];
    const selectedValues = new Set(
      readElicitationAnswerValues(pending, pending.currentQuestionIndex),
    );
    const options: SelectionPrompt["options"] =
      question?.options.map((option) => ({
        id: option.value,
        label: question.multiSelect
          ? `${selectedValues.has(option.value) ? "[x]" : "[ ]"} ${option.label}`
          : option.label,
        // Plan approval 的说明属于语义元数据；纯文本渠道只展示批准/自定义两个动作，
        // 避免把“退出计划模式”展开成额外正文，保持与 Feishu 卡片一致。
        description:
          pending.renderContext?.kind === "plan_approval" ? undefined : option.description,
      })) ?? [];
    if (pending.renderContext?.kind === "plan_approval") {
      // Bugfix: Feishu provider 会自行补自定义回答表单，但 Telegram/微信依赖共享 selection。
      // Plan approval 必须在这里补入口，避免非卡片渠道只能批准、无法提交修改意见。
      options.push({
        id: BOT_ELICITATION_CUSTOM_OPTION_ID,
        label: msg(locale, "elicitationCustomOption"),
      });
    }
    if (question?.multiSelect) {
      options.push({
        id: BOT_ELICITATION_SUBMIT_OPTION_ID,
        label: msg(locale, "elicitationSubmitOption"),
      });
    } else if (question) {
      options.push({
        id: getPendingElicitationSkipOptionId(pending),
        label: msg(locale, "elicitationSkipOption"),
      });
    }
    return {
      id: `elicitation-${pending.requestId}-${pending.currentQuestionIndex}`,
      token: getPendingElicitationSelectionToken(pending),
      title: formatBotElicitationTitle(pending, locale),
      action: "elicitation.respond",
      options,
    };
  }

  async function createElicitationReply(
    actor: BotActor,
    pending: BotPendingElicitation,
    locale: Locale | undefined,
    status: NonNullable<BotOutboundMessage["elicitation"]>["status"] = "pending",
  ): Promise<BotOutboundMessage[]> {
    const currentQuestionIndex =
      status === "pending"
        ? pending.currentQuestionIndex
        : Math.max(0, pending.questions.length - 1);
    return createSelectionReply(actor, createBotElicitationSelection(pending, locale), locale, {
      locale,
      elicitation: {
        requestId: pending.requestId,
        taskId: pending.taskId,
        runId: pending.runId,
        currentQuestionIndex,
        questions: pending.questions,
        answers: pending.answers,
        status,
        ...(pending.expandedCustomAnswerQuestionIndexes?.length
          ? {
              expandedCustomAnswerQuestionIndexes: pending.expandedCustomAnswerQuestionIndexes,
            }
          : {}),
        ...(createPendingElicitationSchema(pending)
          ? { schema: createPendingElicitationSchema(pending) }
          : {}),
      },
    });
  }

  function createCompletedElicitationOutbound(
    actor: BotActor,
    pending: BotPendingElicitation,
    locale: Locale | undefined,
    action: "accept" | "decline" | "cancel",
  ): BotOutboundMessage {
    const status = action === "cancel" ? "cancelled" : "completed";
    return createOutbound(
      actor,
      msg(locale, action === "accept" ? "elicitationSubmitted" : "elicitationCancelled"),
      undefined,
      {
        locale,
        elicitation: {
          requestId: pending.requestId,
          taskId: pending.taskId,
          runId: pending.runId,
          currentQuestionIndex: Math.max(0, pending.questions.length - 1),
          questions: pending.questions,
          answers: pending.answers,
          status,
          ...(createPendingElicitationSchema(pending)
            ? { schema: createPendingElicitationSchema(pending) }
            : {}),
        },
      },
    );
  }

  async function clearPendingElicitationForRequest(
    context: BotContextState,
    requestId: string,
  ): Promise<void> {
    if (context.pendingElicitation?.requestId !== requestId) {
      return;
    }
    clearPendingElicitationSelection(context.pendingElicitation);
    await writeContext({ ...context, pendingElicitation: undefined });
  }

  async function submitPendingElicitation(
    auth: {
      bot: BotConfig;
      context: BotContextState;
      locale: Locale | undefined;
    },
    actor: BotActor,
    pending: BotPendingElicitation,
    action: "accept" | "decline" | "cancel",
    content?: Record<string, unknown>,
  ): Promise<BotOutboundMessage[]> {
    if (!isPendingElicitationOwnedByActor(pending, actor)) {
      return [createOutbound(actor, msg(auth.locale, "elicitationExpired"))];
    }
    if (pending.handledAt) {
      return [createOutbound(actor, msg(auth.locale, "elicitationHandled"))];
    }
    const modeTaskService = await resolveModeTaskServiceForContext(auth.context);
    const submitted = await modeTaskService.respondElicitation({
      taskId: pending.taskId,
      workspacePath: auth.context.workspacePath,
      workspaceIdentity: auth.context.workspaceIdentity,
      runId: pending.runId,
      requestId: pending.requestId,
      action,
      content,
    });
    // 修复原因：v4 resolveInteraction 才是业务确认点。若在 ACK 前写 handledAt，
    // 瞬时失败后的同一按钮重试会被误判为已处理，Agent 将永久停在等待用户输入。
    const handledAt = Date.now();
    await writeContext({
      ...auth.context,
      pendingElicitation: { ...pending, handledAt },
    });
    clearPendingElicitationSelection(pending);
    await writeContext({ ...auth.context, pendingElicitation: undefined });
    await broadcastTaskListChange(auth.context, pending.taskId, "elicitation_resolved", {
      requestId: pending.requestId,
    });
    if (!submitted) {
      return [createOutbound(actor, msg(auth.locale, "elicitationHandled"))];
    }
    if (action === "accept") {
      startTyping(auth.bot, actor, pending.taskId);
      // 与 permission.respond 同理：问答提交后任务恢复，重新通知 started，
      // 避免恢复后的出站失去 stream 归属。
      providers[auth.bot.provider]?.notifyTaskLifecycle?.(auth.bot, actor, "started");
      // Bugfix: AskUserQuestion 只是在回复问题，不属于命令配置成功；这里保留原问答提交文案，避免误回 /status。
      return [createCompletedElicitationOutbound(actor, pending, auth.locale, action)];
    }
    // Bugfix: 取消/拒绝问答也应使用问答自己的结果文案，避免第三方 Bot 里出现无关的任务状态。
    return [createCompletedElicitationOutbound(actor, pending, auth.locale, action)];
  }

  async function advancePendingElicitation(
    auth: {
      bot: BotConfig;
      context: BotContextState;
      locale: Locale | undefined;
    },
    actor: BotActor,
    pending: BotPendingElicitation,
    answers: BotPendingElicitation["answers"],
  ): Promise<BotOutboundMessage[]> {
    if (pending.currentQuestionIndex >= pending.questions.length - 1) {
      return submitPendingElicitation(
        auth,
        actor,
        { ...pending, answers },
        "accept",
        buildBotElicitationContent(pending, answers),
      );
    }
    const nextPending: BotPendingElicitation = {
      ...pending,
      currentQuestionIndex: pending.currentQuestionIndex + 1,
      answers,
    };
    await writeContext({ ...auth.context, pendingElicitation: nextPending });
    // Bugfix: Bot 侧代选 AskUserQuestion 后，UI 只收到最终响应会停留在旧本地草稿。
    // 每次推进题号都同步当前题号和已选答案，让桌面/移动 Web 能保持同一选中态。
    await broadcastPendingElicitationProgress(auth.context, nextPending);
    return createElicitationReply(actor, nextPending, auth.locale);
  }

  async function handlePendingElicitationValue(
    auth: {
      bot: BotConfig;
      context: BotContextState;
      locale: Locale | undefined;
    },
    actor: BotActor,
    value: string,
  ): Promise<BotOutboundMessage[]> {
    const pending = auth.context.pendingElicitation;
    if (!pending || pending.taskId !== auth.context.activeTaskId) {
      return [createOutbound(actor, msg(auth.locale, "elicitationExpired"))];
    }
    if (!isPendingElicitationOwnedByActor(pending, actor)) {
      return [createOutbound(actor, msg(auth.locale, "elicitationExpired"))];
    }
    const question = pending.questions[pending.currentQuestionIndex];
    if (!question) {
      return [createOutbound(actor, msg(auth.locale, "elicitationExpired"))];
    }
    const parsedValue = parseElicitationResponseValue(value);
    if (actor.provider !== "weixin") {
      const expectedToken = getPendingElicitationSelectionToken(pending);
      if (!parsedValue.token || parsedValue.token !== expectedToken) {
        // Bugfix: Telegram/飞书/Webhook 的旧按钮可能在新一轮 AskUserQuestion 后才送达。
        // 非微信通道必须带本轮短 token，避免把上一轮按钮编号误当成当前问题的答案。
        return [createOutbound(actor, msg(auth.locale, "elicitationExpired"))];
      }
    }
    if (parsedValue.value === getPendingElicitationSkipOptionId(pending)) {
      return advancePendingElicitation(auth, actor, pending, pending.answers);
    }
    const formValues = parseElicitationFormValues(parsedValue.value);
    if (formValues) {
      const answerKey = getElicitationAnswerKey(pending.currentQuestionIndex);
      const nextValues = mergeElicitationFormValues(
        question,
        readElicitationAnswerValues(pending, pending.currentQuestionIndex),
        formValues,
      );
      if (nextValues.length === 0) {
        return createElicitationReply(actor, pending, auth.locale);
      }
      // Bugfix: 飞书/Lark 平铺选项由按钮维护草稿，表单只负责提交和自定义输入。
      // 提交时需要合并当前 radio/checkbox 草稿和自定义输入，避免空表单把已选项覆盖掉。
      return advancePendingElicitation(auth, actor, pending, {
        ...pending.answers,
        [answerKey]: nextValues,
      });
    }
    const selectionOption = resolvePendingSelectionOption(
      actor,
      "elicitation.respond",
      parsedValue.value,
    );
    if (selectionOption?.id === getPendingElicitationSkipOptionId(pending)) {
      return advancePendingElicitation(auth, actor, pending, pending.answers);
    }
    const selectedValue = resolveElicitationQuestionValue(
      question,
      selectionOption?.id ?? parsedValue.value,
    );
    if (!selectedValue) {
      return createElicitationReply(actor, pending, auth.locale);
    }
    if (selectedValue === BOT_ELICITATION_SUBMIT_OPTION_ID) {
      return advancePendingElicitation(auth, actor, pending, pending.answers);
    }
    if (selectedValue === BOT_ELICITATION_CUSTOM_OPTION_ID) {
      const nextPending = toggleElicitationCustomAnswerExpanded(pending);
      await writeContext({ ...auth.context, pendingElicitation: nextPending });
      await broadcastPendingElicitationProgress(auth.context, nextPending);
      return createElicitationReply(actor, nextPending, auth.locale);
    }
    const answerKey = getElicitationAnswerKey(pending.currentQuestionIndex);
    if (question.multiSelect) {
      const currentValues = readElicitationAnswerValues(pending, pending.currentQuestionIndex);
      const nextValues = currentValues.includes(selectedValue)
        ? currentValues.filter((item) => item !== selectedValue)
        : [...currentValues, selectedValue];
      const nextPending = {
        ...pending,
        answers: { ...pending.answers, [answerKey]: nextValues },
      };
      await writeContext({ ...auth.context, pendingElicitation: nextPending });
      // Bugfix: 多选题在 Bot 里 toggle 后不会触发 Mode Agent response，必须主动同步草稿给 UI。
      await broadcastPendingElicitationProgress(auth.context, nextPending);
      return createElicitationReply(actor, nextPending, auth.locale);
    }
    return advancePendingElicitation(auth, actor, pending, {
      ...pending.answers,
      [answerKey]: [selectedValue],
    });
  }

  async function handlePendingElicitationText(
    auth: {
      bot: BotConfig;
      context: BotContextState;
      locale: Locale | undefined;
    },
    actor: BotActor,
    text: string,
  ): Promise<BotOutboundMessage[] | null> {
    const pending = auth.context.pendingElicitation;
    if (!pending || pending.taskId !== auth.context.activeTaskId) {
      return null;
    }
    if (!isPendingElicitationOwnedByActor(pending, actor)) {
      return null;
    }
    const value = text.trim();
    if (!value) {
      return createElicitationReply(actor, pending, auth.locale);
    }
    if (value === getPendingElicitationSkipOptionId(pending)) {
      return advancePendingElicitation(auth, actor, pending, pending.answers);
    }
    const answerKey = getElicitationAnswerKey(pending.currentQuestionIndex);
    const question = pending.questions[pending.currentQuestionIndex];
    if (!question) {
      return [createOutbound(actor, msg(auth.locale, "elicitationExpired"))];
    }
    if (
      !question.multiSelect &&
      /^[1-9]\d*$/u.test(value) &&
      Number.parseInt(value, 10) === question.options.length + 1
    ) {
      // 修复原因：自由文本必须保留为用户数据；只有菜单显示的额外序号才是跳过，
      // 避免吞掉名为 skip/next/跳过/__skip__ 的合法选项或自定义答案。
      return advancePendingElicitation(auth, actor, pending, pending.answers);
    }
    const values = question?.multiSelect
      ? value
          .split(/[,\n，、]/u)
          .map((item) => item.trim())
          .filter(Boolean)
          .map((item) =>
            resolveElicitationQuestionValue(question, item, {
              includeSubmit: false,
            }),
          )
      : [resolveElicitationQuestionValue(question, value, { includeSubmit: false })];
    return advancePendingElicitation(auth, actor, pending, {
      ...pending.answers,
      [answerKey]: values,
    });
  }

  async function handleStructuredElicitationResponse(
    message: BotInboundMessage,
    response: BotStructuredElicitationResponse,
  ): Promise<BotOutboundMessage[]> {
    const auth = await withAuthorizedContext(message, "message");
    if (!auth.ok) return auth.reply;
    const pending = auth.context.pendingElicitation;
    if (!pending || pending.requestId !== response.requestId) {
      return [createOutbound(message.actor, msg(auth.locale, "elicitationExpired"))];
    }
    if (!isPendingElicitationOwnedByActor(pending, message.actor)) {
      return [createOutbound(message.actor, msg(auth.locale, "elicitationExpired"))];
    }
    return submitPendingElicitation(
      auth,
      message.actor,
      pending,
      response.action,
      response.content,
    );
  }

  async function handleElicitationRequest(
    bot: BotConfig,
    user: BotConfig,
    actor: BotActor,
    context: BotContextState,
    event: Extract<ModeStreamEvent, { type: "elicitation_request" }>,
  ): Promise<void> {
    const locale = await readMessageLocale();
    stopTyping(event.taskId, bot.id);
    const pendingElicitation: BotPendingElicitation = {
      taskId: event.taskId,
      requestId: event.requestId,
      runId: event.traceId,
      // Bugfix：subagent 发起的 elicitation 必须保留 origin；否则 Bot 广播和后续响应
      // 无法还原请求归属，rebase 后只剩顶层 stream event 带 origin。
      ...(event.origin ? { origin: event.origin } : {}),
      actorKey: getActorContextKey(actor),
      currentQuestionIndex: 0,
      // 修复原因：ExitPlanMode 的协议问题和选项使用稳定英文；如果直接复用，中文 Bot 卡片会中英混杂。
      // Bot 在出站边界按 App locale 本地化整组审批文案，普通 AskUserQuestion 保持模型原文。
      questions: normalizeBotElicitationQuestions(event, locale),
      answers: {},
      // 修复原因：Feishu/Lark 会在自定义回答、完成和重启恢复时重建原卡片；
      // 只把 schema 作为首次发送参数会让后续更新丢失 plan 并退回通用 Question 卡片。
      ...(readBotElicitationRenderContext(event)
        ? { renderContext: readBotElicitationRenderContext(event) }
        : {}),
    };
    // Bugfix: Bot 原先只消费 permission_request，没有把 Mode Agent 的
    // AskUserQuestion/elicitation_request 转成第三方可回答消息，任务会一直卡在等待用户输入。
    if (context.pendingElicitation) {
      clearPendingElicitationSelection(context.pendingElicitation);
    }
    Object.assign(context, { pendingElicitation });
    await writeContext({ ...context, pendingElicitation });
    await broadcastPendingElicitationProgress(context, pendingElicitation);
    for (const reply of await createElicitationReply(actor, pendingElicitation, locale)) {
      if (shouldUseTransientInteractionCard(bot, user)) {
        await upsertTransientInteractionCard(bot, actor, event.taskId, reply);
      } else {
        await sendOutbound(bot, reply);
      }
    }
  }

  async function watchTaskStream(
    bot: BotConfig,
    actor: BotActor,
    context: BotContextState,
    user: BotConfig,
    /** 心跳回合：强制摘要模式，并在整段回复仅含 HEARTBEAT_OK 时保持安静。 */
    heartbeat = false,
  ): Promise<void> {
    if (!context.activeTaskId) {
      return;
    }
    const streamSubscriptionKey = buildTaskStreamSubscriptionKey(
      context.workspacePath,
      context.workspaceIdentity,
      context.activeTaskId,
      bot.id,
    );
    if (streamSubscriptions.has(streamSubscriptionKey)) {
      return;
    }
    // 只撤自己这条订阅（key 在建立时确定；上下文若已迁走，writeContext 已把它撤掉）。
    const disposeOwnSubscription = () => {
      streamSubscriptions.get(streamSubscriptionKey)?.dispose();
      streamSubscriptions.delete(streamSubscriptionKey);
    };
    let assistantParts: ModeAssistantMessagePart[] = [];
    let assistantReplyBuffer = "";
    let sentAnyAssistantReply = false;
    const assistantPartToolIds = new Set<string>();
    const toolCalls = new Map<string, BotReplyToolCallState>();
    const sentToolCallReplyIds = new Set<string>();
    // 群聊「全部消息」模式：模型可用 <NO_REPLY> 主动保持沉默。
    // 这类回合不出流式卡片、也不发中途工具摘要，沉默时用户侧完全无痕
    // （无需撤回任何中间态；代价是这类回合没有打字机效果）。
    const groupSilenceCapable =
      actor.chatType === "group" && (bot.groupChat?.activation ?? "disabled") === "always";
    const getMode = () =>
      heartbeat ? "summary_changes" : normalizeBotReplyGranularity(bot.provider, user.replyMode);
    let streamingCardHandle: BotStreamingReplyCardHandle | null = null;
    let streamingCardSegmentIndex = 0;
    const streamingCardBlocks: StreamingCardTimelineBlock[] = [];
    let streamingCardStatus: "running" | "sealed" | "completed" | "error" = "running";
    let streamingCardLastUpdateAt = 0;
    let streamingCardConsecutiveFailures = 0;
    let streamingCardNextAttemptAt = 0;
    let streamingCardCircuitOpen = false;
    let streamingCardQueue: Promise<void> = Promise.resolve();
    const supportsStreamingCardReply = () => {
      const adapter = providers[bot.provider];
      // 可能沉默的回合（群聊「全部消息」）不建卡片：沉默时不留下需要撤回的空卡片。
      if (groupSilenceCapable) {
        return false;
      }
      // 能力协商由 adapter 决定：飞书走 Card JSON 2.0，Telegram 走 sendMessage+editMessageText
      // 的打字机编辑，企微走 replyStream，钉钉走 AI 卡片。
      if (getMode() !== "streaming_card") {
        return false;
      }
      // 钉钉卡片流式依赖「开关 + 模板 ID」；未配置时回退 Markdown 摘要，
      // 而不是让 create 返回 null 进入失败退避（那会把终稿一起熔断掉）。
      if (bot.provider === "dingtalk" && !isDingtalkAiCardEnabled(bot)) {
        return false;
      }
      return Boolean(adapter?.createStreamingReplyCard) && Boolean(adapter?.updateStreamingReplyCard);
    };
    const buildStreamingToolSummaryTitle = (locale: Locale | undefined): string =>
      msg(locale, "streamingToolSummaries");
    const appendStreamingCardMessageChunk = (content: string): void => {
      if (!content) {
        return;
      }
      const lastBlock = streamingCardBlocks.at(-1);
      if (lastBlock?.type === "message") {
        lastBlock.text += content;
        return;
      }
      streamingCardBlocks.push({ type: "message", text: content });
    };
    const appendStreamingCardMessages = (messages: readonly string[]): void => {
      for (const message of messages.map((item) => item.trim()).filter(Boolean)) {
        const lastBlock = streamingCardBlocks.at(-1);
        if (lastBlock?.type === "message" && lastBlock.text.trim()) {
          lastBlock.text = `${lastBlock.text.trim()}\n\n${message}`;
        } else {
          streamingCardBlocks.push({ type: "message", text: message });
        }
      }
    };
    const hasStreamingCardMessageText = (): boolean =>
      streamingCardBlocks.some((block) => block.type === "message" && block.text.trim().length > 0);
    const appendStreamingCardTool = (toolId: string): void => {
      const existingBlock = streamingCardBlocks.find(
        (block) => block.type === "tools" && block.toolIds.includes(toolId),
      );
      if (existingBlock) {
        return;
      }
      const lastBlock = streamingCardBlocks.at(-1);
      if (lastBlock?.type === "tools") {
        lastBlock.toolIds.push(toolId);
        return;
      }
      streamingCardBlocks.push({ type: "tools", toolIds: [toolId] });
    };
    const buildStreamingCardBlocks = (locale: Locale | undefined): BotStreamingReplyCardBlock[] => {
      // 工具摘要只作为"进行中"的过程提示：终稿（completed/sealed）里不再出现，
      // 交付内容与「标准回复」保持一致（只留助手正文与文件变更摘要）。
      const running = streamingCardStatus === "running";
      const toolSummaryTitle = buildStreamingToolSummaryTitle(locale);
      const latestToolBlockIndex = streamingCardBlocks.reduce(
        (latestIndex, block, index) => (block.type === "tools" ? index : latestIndex),
        -1,
      );
      const blocks: BotStreamingReplyCardBlock[] = [];
      for (const [index, block] of streamingCardBlocks.entries()) {
        if (block.type === "message") {
          const text = block.text.trim();
          if (text) {
            blocks.push({ type: "message", text });
          }
          continue;
        }
        if (!running) {
          continue;
        }
        const summaries = block.toolIds
          .map((toolId) => toolCalls.get(toolId))
          .filter((toolCall): toolCall is BotReplyToolCallState => Boolean(toolCall))
          .map((toolCall) =>
            formatBotToolCallSummaryLine(toolCall, {
              workspacePath: context.workspacePath,
              locale,
            }),
          );
        if (summaries.length === 0) {
          continue;
        }
        blocks.push({
          type: "tools",
          title: toolSummaryTitle,
          summaries,
          expanded: streamingCardStatus === "running" && index === latestToolBlockIndex,
        });
      }
      if (blocks.length === 0) {
        blocks.push({
          type: "message",
          // 进行中且还没输出正文：保留"正在处理"；终稿没有任何内容（纯工具回合且无变更）：
          // 用完成提示兜底，避免把最后一条过程提示当成终稿。
          text: msg(locale, running ? "streamingWorking" : "taskCompleted"),
        });
      }
      return blocks;
    };
    const syncStreamingCardReply = async (trigger: string, force = false): Promise<void> => {
      if (!supportsStreamingCardReply()) {
        return;
      }
      const now = Date.now();
      // Bugfix：旧实现只在成功后更新时间基准，Feishu 失败时每个 stream event 都会真实发请求；
      // force 路径还会绕过普通节流。失败退避和熔断必须先于 force 判断，避免单次 400 被放大成风暴。
      if (streamingCardCircuitOpen || now < streamingCardNextAttemptAt) {
        return;
      }
      // 首帧门槛：纯文本增量还没成形时先不创建流式消息，避免出现只有一两个字符的首帧；
      // 工具活动（tool_call 等）不受此限——它本身就要立刻给出"正在处理"的反馈。
      // 任务结束时 force=true 会绕过门槛，短回复因此不会丢内容。
      if (!streamingCardHandle && !force && trigger === "agent_message_chunk") {
        const accumulatedText = streamingCardBlocks
          .filter((block): block is { type: "message"; text: string } => block.type === "message")
          .map((block) => block.text)
          .join("");
        if (!hasSentenceBoundary(accumulatedText)) {
          return;
        }
      }
      if (
        streamingCardHandle &&
        !force &&
        now - streamingCardLastUpdateAt <
          (STREAMING_CARD_MIN_UPDATE_INTERVAL_MS_BY_PROVIDER[bot.provider] ??
            FEISHU_STREAMING_CARD_MIN_UPDATE_INTERVAL_MS)
      ) {
        return;
      }
      const adapter = providers[bot.provider];
      const locale = await readMessageLocale();
      const state = {
        providerUserId: actor.providerUserId,
        locale,
        blocks: buildStreamingCardBlocks(locale),
        status: streamingCardStatus,
      };
      const states = adapter?.splitStreamingReplyCardStates?.(state) ?? [state];
      streamingCardQueue = streamingCardQueue
        .catch(() => undefined)
        .then(async () => {
          let operation = streamingCardHandle ? "update" : "create";
          const requestController = new AbortController();
          streamingCardRequestControllers.add(requestController);
          const timeoutId = setTimeout(() => {
            requestController.abort(new Error("Feishu streaming card request timed out."));
          }, FEISHU_STREAMING_CARD_REQUEST_TIMEOUT_MS);
          try {
            for (
              let index = Math.min(streamingCardSegmentIndex, states.length - 1);
              index < states.length;
              index += 1
            ) {
              const segmentState = states[index]!;
              operation = streamingCardHandle ? "update" : "create";
              const request = !streamingCardHandle
                ? adapter?.createStreamingReplyCard?.(bot, segmentState, requestController.signal)
                : adapter?.updateStreamingReplyCard?.(
                    bot,
                    streamingCardHandle,
                    segmentState,
                    requestController.signal,
                  );
              const result = await Promise.race([
                request,
                new Promise<never>((_, reject) => {
                  requestController.signal.addEventListener(
                    "abort",
                    () => reject(requestController.signal.reason),
                    { once: true },
                  );
                }),
              ]);
              if (!streamingCardHandle) {
                streamingCardHandle = result ?? null;
                if (!streamingCardHandle) {
                  // Bug 根因：飞书创建接口可能 code=0 却不返回 message_id。若把这种静默失败
                  // 当成成功推进 segmentIndex，未投递的中间段会被永久跳过；必须统一进入退避重试。
                  throw new Error("Feishu create streaming card returned no message_id.");
                }
              }
              if (index < states.length - 1) {
                // 修复原因：当前卡片达到飞书元素预算后必须保留为 sealed 历史段，
                // 后续 block 只写入新卡片，不能把已展示内容再次发送或继续更新旧 message_id。
                streamingCardSegmentIndex = index + 1;
                streamingCardHandle = null;
              }
            }
            streamingCardLastUpdateAt = Date.now();
            streamingCardConsecutiveFailures = 0;
            streamingCardNextAttemptAt = 0;
          } catch (error) {
            // Bugfix: 第三方卡片只是 best-effort 展示，超时/失败不能阻塞 task_complete、
            // task_error 或 typing 清理等生命周期事件。
            streamingCardConsecutiveFailures += 1;
            const errorMessage = error instanceof Error ? error.message : String(error);
            if (
              streamingCardConsecutiveFailures >= FEISHU_STREAMING_CARD_FAILURE_CIRCUIT_THRESHOLD
            ) {
              streamingCardCircuitOpen = true;
              botsLogger.warn(
                undefined,
                `Feishu streaming card circuit opened task=${context.activeTaskId} trigger=${trigger} operation=${operation} failures=${streamingCardConsecutiveFailures}: ${errorMessage}`,
              );
            } else {
              const retryDelayMs =
                FEISHU_STREAMING_CARD_FAILURE_BACKOFF_BASE_MS *
                2 ** (streamingCardConsecutiveFailures - 1);
              streamingCardNextAttemptAt = Date.now() + retryDelayMs;
              botsLogger.warn(
                undefined,
                `Feishu streaming card sync failed task=${context.activeTaskId} trigger=${trigger} operation=${operation} failures=${streamingCardConsecutiveFailures} retryDelayMs=${retryDelayMs}: ${errorMessage}`,
              );
            }
          } finally {
            clearTimeout(timeoutId);
            streamingCardRequestControllers.delete(requestController);
          }
        });
      await streamingCardQueue;
    };
    const sealStreamingCardReply = async (): Promise<void> => {
      if (!streamingCardHandle) {
        return;
      }
      // 修复原因：阻塞交互前的 Agent 输出与交互后的 continuation 属于两个可读段落。
      // 旧实现继续复用同一 message_id，导致问题/Plan 卡夹在中间但后续正文回写到旧卡。
      streamingCardStatus = "sealed";
      await syncStreamingCardReply("seal", true);
      streamingCardHandle = null;
      streamingCardSegmentIndex = 0;
      streamingCardBlocks.length = 0;
      streamingCardStatus = "running";
      streamingCardLastUpdateAt = 0;
    };
    const flushAssistantReplyBuffer = async (force = false) => {
      if (
        getMode() === "summary_changes" ||
        supportsStreamingCardReply() ||
        !assistantReplyBuffer
      ) {
        return;
      }
      const extracted = extractBotAssistantResponseMessages(assistantReplyBuffer, force);
      assistantReplyBuffer = extracted.rest;
      for (const text of extracted.messages) {
        sentAnyAssistantReply = true;
        await sendOutbound(bot, createOutbound(actor, text));
      }
    };
    const modeTaskService = await resolveModeTaskServiceForContext(context);
    const handleStreamEvent = async (
      event: ModeStreamEvent | TaskStreamMirrorableEvent,
      shouldBroadcast = true,
    ): Promise<void> => {
      if (event.type === "task_stream_mirror_batch") {
        if (shouldBroadcast) {
          await broadcastTaskStreamEvent(context, event);
        }
        // Bugfix: 共享 host / 远控下 UI 收到的是 workspace 级 mirror batch。
        // 旧逻辑只识别裸 stream event，导致 UI 正常流式显示但 Bot channel 没有任何可发送回复。
        for (const op of event.ops) {
          if (op.kind === "stream_event") {
            await handleStreamEvent(op.event, false);
          }
        }
        return;
      }
      if (shouldBroadcast) {
        await broadcastTaskStreamEvent(context, event);
      }
      // Bugfix: 第三方默认回复需要随 AssistantMessageResponse 流式发送；
      // 但 /status Progress 仍然要独立缓存，避免受发送颗粒度影响。
      updateLiveStatusProgress(event);
      if (event.type === "agent_message_chunk") {
        assistantParts = appendAssistantMessagePart(assistantParts, {
          type: "content",
          content: event.content,
        });
        if (supportsStreamingCardReply()) {
          appendStreamingCardMessageChunk(event.content);
          await syncStreamingCardReply(event.type, false);
          return;
        }
        if (getMode() !== "summary_changes") {
          assistantReplyBuffer += event.content;
          // 第三方平台消息是离散气泡；formatter 负责把当前 buffer 按长度约束拆成可发送消息。
          await flushAssistantReplyBuffer(false);
        }
        return;
      }
      if (event.type === "agent_thought_chunk") {
        assistantParts = appendAssistantMessagePart(assistantParts, {
          type: "thought",
          content: event.content,
        });
      }
      if (event.type === "tool_call" || event.type === "tool_call_update") {
        if (supportsStreamingCardReply()) {
          appendStreamingCardTool(event.toolId);
        }
        if (!supportsStreamingCardReply()) {
          await flushAssistantReplyBuffer(true);
        }
        // Bugfix: summary_changes 完成消息需要参考 UI latestPart。
        // tool_call_update 可能在缺少 tool_call 首帧时先到，需像 UI 一样补一个 tool-call part 边界。
        if (!assistantPartToolIds.has(event.toolId)) {
          assistantPartToolIds.add(event.toolId);
          assistantParts = appendAssistantMessagePart(assistantParts, {
            type: "tool-call",
            toolId: event.toolId,
          });
        }
      }
      updateBotReplyToolCalls(toolCalls, event);
      if (event.type === "tool_call" && supportsStreamingCardReply()) {
        await syncStreamingCardReply(event.type, true);
      }
      if (event.type === "tool_call_update" && supportsStreamingCardReply()) {
        await syncStreamingCardReply(event.type, isBotToolCallReplyTerminal(event.status));
      }
      if (
        event.type === "tool_call_update" &&
        getMode() === "assistant_toolcalls_changes" &&
        isBotToolCallReplyTerminal(event.status) &&
        !sentToolCallReplyIds.has(event.toolId) &&
        // 可能沉默的群聊回合不中途发言：工具摘要会先于 <NO_REPLY> 判断落到群里。
        !groupSilenceCapable
      ) {
        const toolCall = toolCalls.get(event.toolId);
        if (toolCall) {
          sentToolCallReplyIds.add(event.toolId);
          sentAnyAssistantReply = true;
          await sendOutbound(
            bot,
            createOutbound(
              actor,
              formatBotToolCallReply(toolCall, {
                workspacePath: context.workspacePath,
                locale: await readMessageLocale(),
              }),
            ),
          );
        }
      }
      if (event.type === "permission_request") {
        const locale = await readMessageLocale();
        stopTyping(event.taskId, bot.id);
        await sealStreamingCardReply();
        await broadcastTaskListChange(context, event.taskId, "permission_request", {
          permissionRequest: event,
        });
        // Bugfix: UI 会把 Mode Agent 原始权限选项规整成“允许/始终允许/拒绝”的固定顺序和文案；
        // 机器人之前直接展示 provider 原始英文 name，还额外加取消按钮，导致同一个权限请求在飞书和 UI 看起来不一致。
        const permissionOptions = sortBotPermissionOptions(event.options);
        const permissionSelection: SelectionPrompt = {
          id: `permission-${event.requestId}`,
          title: formatBotPermissionRequestSummary(event, {
            locale,
            workspacePath: context.workspacePath,
          }),
          action: "permission.respond",
          showCancel: false,
          options: permissionOptions.map((option) => {
            const isDenyOption = isBotPermissionRejectOption(option);
            return {
              id: isDenyOption
                ? `/deny ${event.requestId}`
                : `/approve ${event.requestId} ${option.optionId}`,
              label: formatBotPermissionOptionLabel(option, locale),
              description: formatBotPermissionOptionDescription(option, event, locale),
            };
          }),
        };
        // Bugfix: Telegram callback_data 只有 64 字节，真实 toolCallId/requestId 可能过长。
        // 因此按钮只回传短序号，真实 requestId/optionId 暂存在当前 bot context 中再解析。
        const pendingPermissionOptions = permissionOptions.map((option) => {
          const isDenyCommand = isBotPermissionRejectOption(option);
          return {
            requestId: event.requestId,
            optionId: option.optionId,
            command: isDenyCommand ? ("deny" as const) : ("approve" as const),
            label: formatBotPermissionOptionLabel(option, locale),
            response: option.response,
          };
        });
        Object.assign(context, { pendingPermissionOptions });
        await writeContext({ ...context, pendingPermissionOptions });
        const [permissionReply] = await createSelectionReply(
          actor,
          permissionSelection,
          await readMessageLocale(),
        );
        if (permissionReply) {
          if (shouldUseTransientInteractionCard(bot, user)) {
            await upsertTransientInteractionCard(bot, actor, event.taskId, permissionReply);
          } else {
            await sendOutbound(bot, permissionReply);
          }
        }
        providers[bot.provider]?.notifyTaskLifecycle?.(bot, actor, "awaiting_input");
        return;
      }
      if (event.type === "elicitation_request") {
        await sealStreamingCardReply();
        await handleElicitationRequest(bot, user, actor, context, event);
        providers[bot.provider]?.notifyTaskLifecycle?.(bot, actor, "awaiting_input");
        return;
      }
      if (event.type === "elicitation_response") {
        await clearPendingElicitationForRequest(context, event.requestId);
        await broadcastTaskListChange(context, event.taskId, "elicitation_resolved", {
          requestId: event.requestId,
        });
        return;
      }
      if (event.type === "task_complete" || event.type === "task_error") {
        runningTasks.delete(event.taskId);
        liveStatusProgressByTaskId.delete(event.taskId);
        stopTyping(event.taskId, bot.id);
        // 终态收口放在 finally：notifyTaskLifecycle 的终态分支会发出 status 终止符并删除
        // 任务流，若在分支开头调用，后面的失败原因、变更摘要、「任务已完成。」与 transient
        // card 收尾都会因找不到 stream 而落到 idFactory() 的新流上（orphan stream）。
        // 约定：status 必须是该 binding 的最后一条帧；终态文案不得落在未 accepted 预告的流上。
        const terminalPhase = event.type === "task_error" ? "failed" : "completed";
        try {
          if (context.pendingElicitation?.taskId === event.taskId) {
            clearPendingElicitationSelection(context.pendingElicitation);
            await writeContext({ ...context, pendingElicitation: undefined });
          }
          const transientCard = transientInteractionCards.get(getActorContextKey(actor));
          if (transientCard?.taskId === event.taskId) {
            const pendingElicitation = context.pendingElicitation;
            await finalizeTransientInteractionCard(
              actor,
              pendingElicitation
                ? createCompletedElicitationOutbound(
                    actor,
                    pendingElicitation,
                    await readMessageLocale(),
                    "cancel",
                  )
                : createOutbound(
                    actor,
                    event.type === "task_error"
                      ? msg(await readMessageLocale(), "taskFailed", {
                          message: event.error,
                        })
                      : msg(await readMessageLocale(), "received"),
                  ),
            );
          }
          // Bugfix: Mode Agent 终态事件可能先于 task index/meta 落盘广播到 Bots。
          // 如果这里立刻用旧 meta 更新 sidebar，随后列表再刷新到终态 meta，会出现状态/摘要跳一下。
          // 因此终态广播前短重试读取一次稳定 meta，尽量用同一帧完成 UI 增量更新。
          const completedTask = await readTerminalTaskMeta(context, event.taskId, event.type).catch(
            () => null,
          );
          await broadcastTaskListChange(
            context,
            event.taskId,
            event.type === "task_error" ? "error" : "completed",
            {
              ...(completedTask ? { task: completedTask } : {}),
              ...(event.type === "task_error" ? { error: event.error } : {}),
            },
          );
          disposeOwnSubscription();
          if (event.type === "task_error") {
            if (supportsStreamingCardReply()) {
              streamingCardStatus = "error";
              if (!hasStreamingCardMessageText()) {
                appendStreamingCardMessages([
                  msg(await readMessageLocale(), "taskFailed", {
                    message: event.error,
                  }),
                ]);
              }
              await syncStreamingCardReply(event.type, true);
              return;
            }
            await sendOutbound(
              bot,
              createOutbound(
                actor,
                msg(await readMessageLocale(), "taskFailed", {
                  message: event.error,
                }),
              ),
            );
            return;
          }

          // 心跳回合的安静回执：整段回复只有 HEARTBEAT_OK 时不打扰用户。
          if (heartbeat) {
            const heartbeatText = assistantParts
              .filter((part) => part.type === "content")
              .map((part) => (part.type === "content" ? part.content : ""))
              .join("");
            if (isHeartbeatOkOnly(heartbeatText)) {
              disposeOwnSubscription();
              return;
            }
          }
          // 群聊「全部消息」模式的沉默回执：整段回复只有 <NO_REPLY> 时不发任何消息。
          // 该回合没有卡片、没有中途摘要，因此群里完全无痕。
          if (groupSilenceCapable) {
            const groupText = assistantParts
              .filter((part) => part.type === "content")
              .map((part) => (part.type === "content" ? part.content : ""))
              .join("");
            if (isGroupSilenceReply(groupText)) {
              return;
            }
          }
          const mode = getMode();
          const locale = await readMessageLocale();
          const completedSnapshot = await modeTaskService
            .getTaskSnapshot({
              taskId: event.taskId,
              workspacePath: context.workspacePath,
              workspaceIdentity: context.workspaceIdentity,
            })
            .catch(() => null);
          const latestTurnChangeSummary = readLatestAssistantTurnChangeSummary(completedSnapshot);
          if (supportsStreamingCardReply()) {
            const changeSummaryMessages = formatBotAssistantReplyBlocks(
              createAssistantReplyBlocks([], new Map(), mode, latestTurnChangeSummary),
              {
                workspacePath: context.workspacePath,
                locale,
              },
            );
            if (changeSummaryMessages.length > 0) {
              appendStreamingCardMessages(changeSummaryMessages);
            }
            streamingCardStatus = "completed";
            await syncStreamingCardReply(event.type, true);
            sentAnyAssistantReply = true;
            return;
          }
          let replyMessages: string[] = [];
          if (mode === "summary_changes") {
            const replyBlocks = createAssistantReplyBlocks(
              assistantParts,
              toolCalls,
              mode,
              latestTurnChangeSummary,
            );
            replyMessages = formatBotAssistantReplyBlocks(replyBlocks, {
              workspacePath: context.workspacePath,
              locale,
            });
          } else {
            await flushAssistantReplyBuffer(true);
            const changeSummaryBlocks = createAssistantReplyBlocks(
              [],
              new Map(),
              mode,
              latestTurnChangeSummary,
            );
            replyMessages = formatBotAssistantReplyBlocks(changeSummaryBlocks, {
              workspacePath: context.workspacePath,
              locale,
            });
          }
          if (replyMessages.length === 0 && !sentAnyAssistantReply) {
            await sendOutbound(
              bot,
              createOutbound(actor, locale === "en-US" ? "Task completed." : "任务已完成。"),
            );
            return;
          }
          for (const text of replyMessages) {
            sentAnyAssistantReply = true;
            await sendOutbound(bot, createOutbound(actor, text));
          }
        } finally {
          providers[bot.provider]?.notifyTaskLifecycle?.(bot, actor, terminalPhase);
          // 运行期间排队的消息在终态收口后按序投递；fire-and-forget，不阻塞流事件队列。
          void drainQueuedMessages(bot, event.taskId).catch((error: unknown) => {
            botsLogger.warn(
              undefined,
              `queued message drain failed bot=${bot.id}: ${
                error instanceof Error ? error.message : String(error)
              }`,
            );
          });
        }
      }
    };
    let streamEventQueue: Promise<void> = Promise.resolve();
    const enqueueStreamEvent = (
      event: ModeStreamEvent | TaskStreamMirrorableEvent,
    ): Promise<void> => {
      const nextStreamEvent = streamEventQueue.then(() => handleStreamEvent(event));
      // Bugfix: Mode Agent 事件分发不保证等待 async listener。微信这类离散消息如果并发发送，
      // task_complete 的 Change summary 可能抢在前面正文 flush 之前到达客户端，所以这里按任务串行消费。
      streamEventQueue = nextStreamEvent.catch((error: unknown) => {
        botsLogger.warn(
          event.traceId,
          `bot task stream event failed task=${event.taskId}: ${
            error instanceof Error ? error.message : String(error)
          }`,
        );
      });
      return streamEventQueue;
    };
    const dynamicTaskEvent = (
      modeTaskService as Partial<Pick<IModeTaskService, "onDynamicTaskEvent">>
    ).onDynamicTaskEvent;
    // Bugfix: 远控/共享 host 场景会通过 workspace+task mirror 分发流事件。
    // 这里优先订阅 workspace 级事件，避免只监听本地 taskId relay 时漏掉 channel 回复。
    const streamDisposable = dynamicTaskEvent
      ? dynamicTaskEvent({
          workspacePath: context.workspacePath,
          workspaceIdentity: context.workspaceIdentity,
          taskId: context.activeTaskId,
          // Bugfix: Bot channel 使用 direct stream 语义。
          // 手机远控 replayable 的 mirror replay / snapshot gap recovery 会改变 bot 回复边界，
          // 这里使用 bot 专属 continuous 订阅，避免远控恢复逻辑影响飞书/微信等 channel。
          deliveryKind: "bot-channel-continuous",
        })(enqueueStreamEvent)
      : modeTaskService.onDynamicStreamEvent(context.activeTaskId)(enqueueStreamEvent);
    streamSubscriptions.set(streamSubscriptionKey, {
      dispose() {
        streamDisposable.dispose();
      },
    });
    startTyping(bot, actor, context.activeTaskId);
    // 告诉传输型 provider 已进入任务流：此时不得提前收口 bridge 轮次。
    providers[bot.provider]?.notifyTaskLifecycle?.(bot, actor, "started");
  }

  async function createSelectionReply(
    actor: BotActor,
    selection: SelectionPrompt,
    locale?: Locale,
    extras: Pick<BotOutboundMessage, "elicitation" | "locale"> = {},
  ): Promise<BotOutboundMessage[]> {
    const markedSelection = markCurrentSelection(selection, locale);
    // Bugfix: 微信没有原生选项卡能力，只能走纯文本编号选项。
    // 之前纯文本 fallback 会同时展示标题里的“当前”和选项上的“当前”标记，
    // 微信回复看起来像重复状态文案；这里让标题负责说明当前状态，列表只保留可回复的编号。
    const supportsStructuredSelection = actor.provider !== "weixin";
    // Bugfix: 微信 /model 第一层选择的是供应商，之前复用 description 把模型列表也拼进同一行，
    // 导致用户还没选供应商就看到两层信息。纯文本通道先只展示供应商，模型放到下一层再展示。
    const textSelection = stripModelProviderDescriptionsForTextSelection(selection);
    const displaySelection = supportsStructuredSelection
      ? markedSelection
      : { ...textSelection, cancelLabel: markedSelection.cancelLabel };
    pendingSelectionsByContext.set(getActorContextKey(actor), displaySelection);
    // 其他 provider 保留 selection，让 Telegram/飞书/Lark 渲染原生选项，也让 Webhook 接收结构化选项。
    const text = supportsStructuredSelection
      ? displaySelection.title
      : formatSelectionFallback(displaySelection, locale);
    return [
      createOutbound(actor, text, supportsStructuredSelection ? displaySelection : undefined, {
        ...extras,
        locale,
      }),
    ];
  }

  async function handleSelectionCancel(message: BotInboundMessage): Promise<BotOutboundMessage[]> {
    const locale = await readMessageLocale();
    const actorContextKey = getActorContextKey(message.actor);
    const pendingSelection = pendingSelectionsByContext.get(actorContextKey);
    if (pendingSelection?.action === "elicitation.respond") {
      const auth = await withAuthorizedContext(message, "message");
      if (!auth.ok) return auth.reply;
      const pending = auth.context.pendingElicitation;
      if (!pending) {
        clearPendingSelection(message.actor);
        return [createOutbound(message.actor, msg(auth.locale, "elicitationExpired"))];
      }
      clearPendingSelection(message.actor);
      return submitPendingElicitation(auth, message.actor, pending, "cancel");
    }
    if (!pendingSelectionsByContext.has(actorContextKey)) {
      const auth = await withAuthorizedContext(message, "message");
      if (auth.ok && auth.context.pendingElicitation) {
        return submitPendingElicitation(
          auth,
          message.actor,
          auth.context.pendingElicitation,
          "cancel",
        );
      }
      return [createOutbound(message.actor, msg(locale, "unknownCommand", { command: "0" }))];
    }
    clearPendingSelection(message.actor);
    const auth = await withAuthorizedContext(message, "message");
    if (!auth.ok) return auth.reply;
    return createStatusReply(message.actor, auth.context, auth.locale);
  }

  function buildRemoteReconnectCommandKey(
    actor: BotActor,
    context: Pick<BotContextState, "workspacePath" | "workspaceIdentity">,
  ): string {
    return [
      actor.botId,
      actor.provider,
      actor.chatId ?? actor.providerUserId,
      getWorkspaceKey(context.workspacePath, context.workspaceIdentity),
    ].join("::");
  }

  function buildRemoteReconnectDeliveryKey(message: BotInboundMessage): string | null {
    const providerMessageId = message.actor.providerMessageId?.trim();
    if (!providerMessageId) {
      return null;
    }
    return [
      message.actor.botId,
      message.actor.provider,
      message.actor.chatId ?? message.actor.providerUserId,
      providerMessageId,
    ].join("::");
  }

  function pruneRecentRemoteReconnectDeliveryDedupe(now: number): void {
    for (const [key, at] of recentRemoteReconnectDeliveryAtByKey) {
      if (now - at >= REMOTE_RECONNECT_DELIVERY_DEDUPE_TTL_MS) {
        recentRemoteReconnectDeliveryAtByKey.delete(key);
      }
    }
  }

  async function performRemoteReconnect(
    message: BotInboundMessage,
    auth: Extract<Awaited<ReturnType<typeof withAuthorizedContext>>, { ok: true }>,
  ): Promise<BotOutboundMessage[]> {
    let result: BotRemoteWorkspaceReconnectResult;
    try {
      result = await reconnectRemoteWorkspaceForBot(auth.context);
    } catch (error) {
      result = {
        ok: false,
        message: error instanceof Error ? error.message : String(error),
      };
    }
    if (!result.ok) {
      return [
        createOutbound(
          message.actor,
          msg(auth.locale, "remoteReconnectFailed", {
            workspacePath: auth.context.workspacePath,
            message: result.message ?? "unknown",
          }),
        ),
      ];
    }
    if (auth.context.mode === "draft" || !auth.context.activeTaskId) {
      const draftOptions = await buildInitializedDraftOptions(auth.context);
      await writeContext({ ...auth.context, draftOptions });
    }
    // 成功重连后统一回完整状态，避免命令完成文案和 /status 内容分裂。
    return createStatusReply(message.actor, auth.context, auth.locale);
  }

  async function handleReconnect(
    message: BotInboundMessage,
    options: {
      onReconnectStart?: (
        auth: Extract<Awaited<ReturnType<typeof withAuthorizedContext>>, { ok: true }>,
      ) => Promise<void>;
    } = {},
  ): Promise<BotOutboundMessage[]> {
    const auth = await withAuthorizedContext(message, "workspace");
    if (!auth.ok) return auth.reply;
    if (!auth.context.workspaceIdentity) {
      return [createOutbound(message.actor, msg(auth.locale, "remoteReconnectLocal"))];
    }
    if (!deps.remoteWorkspaceService) {
      return [
        createOutbound(
          message.actor,
          msg(auth.locale, "remoteReconnectUnavailable", {
            workspacePath: auth.context.workspacePath,
          }),
        ),
      ];
    }

    const now = Date.now();
    pruneRecentRemoteReconnectDeliveryDedupe(now);
    const deliveryKey = buildRemoteReconnectDeliveryKey(message);
    if (deliveryKey && recentRemoteReconnectDeliveryAtByKey.has(deliveryKey)) {
      return [];
    }
    if (deliveryKey) {
      // Bugfix: 飞书/微信/Telegram 都可能重投同一条 provider message。
      // /reconnect 有副作用，必须在真正执行前就按 provider message id 幂等，
      // 否则重投会再次命中“已连接”分支，用户会看到重复的成功提示。
      recentRemoteReconnectDeliveryAtByKey.set(deliveryKey, now);
    }

    const reconnectKey = buildRemoteReconnectCommandKey(message.actor, auth.context);
    const pendingReconnect = pendingRemoteReconnectsByKey.get(reconnectKey);
    if (pendingReconnect) {
      await pendingReconnect.catch(() => []);
      return [];
    }
    const recentReconnectAt = recentRemoteReconnectAtByKey.get(reconnectKey);
    if (
      recentReconnectAt !== undefined &&
      now - recentReconnectAt < REMOTE_RECONNECT_DEDUPE_TTL_MS
    ) {
      return [];
    }
    if (await isRemoteWorkspaceConnected(auth.context)) {
      return createStatusReply(message.actor, auth.context, auth.locale);
    }

    // Bugfix: Feishu/Lark/Webhook 这类 provider 可能把同一条 /reconnect 在短时间内重复投递。
    // /reconnect 本身有副作用，必须按 bot+用户+workspace 做幂等，否则会同时出现“已连接”和“正在重连”等互相打架的状态。
    const reconnectPromise = (async () => {
      if (options.onReconnectStart) {
        try {
          await options.onReconnectStart(auth);
        } catch (error) {
          // Bugfix: “正在重连”只是即时状态提示，发送失败不能中断真正的远端重连。
          botsLogger.warn(
            undefined,
            `send reconnect starting failed provider=${message.actor.provider} bot=${message.botId} user=${message.actor.providerUserId}: ${error instanceof Error ? error.message : String(error)}`,
          );
        }
      }
      return performRemoteReconnect(message, auth);
    })();
    pendingRemoteReconnectsByKey.set(reconnectKey, reconnectPromise);
    try {
      const replies = await reconnectPromise;
      recentRemoteReconnectAtByKey.set(reconnectKey, Date.now());
      return replies;
    } finally {
      pendingRemoteReconnectsByKey.delete(reconnectKey);
    }
  }

  async function withAuthorizedContext(
    message: BotInboundMessage,
    requestedCommand: BotAuthorizedCommand,
  ): Promise<
    | {
        ok: true;
        config: BotsConfigFile;
        bot: BotConfig;
        user: BotConfig;
        context: BotContextState;
        locale: Locale | undefined;
      }
    | { ok: false; reply: BotOutboundMessage[] }
  > {
    const locale = await readMessageLocale();
    const config = await repo.readConfig();
    const bot = findAuthorizedBot(config, message.actor);
    const privateChatAllUsers = (bot?.privateChatMode ?? "bound_users") === "all_users";
    if (!bot || bot.id !== message.botId) {
      return {
        ok: false,
        reply: [createOutbound(message.actor, msg(locale, "botDisabled"))],
      };
    }
    if (message.actor.chatType !== "private") {
      const activation = bot.groupChat?.activation ?? "disabled";
      if (activation === "disabled") {
        return {
          ok: false,
          reply: [createOutbound(message.actor, msg(locale, "privateChatOnly"))],
        };
      }
      // 群聊服务哪些人：私聊方式=全部用户时任何群成员都可以驱动；否则只服务绑定用户。
      // 未授权成员的消息记入上下文后静默忽略，既不外泄"这不是私聊"的提示，也不给别人刷机器人的机会。
      const boundUserId = bot.providerUserId?.trim();
      const isBoundSender =
        privateChatAllUsers ||
        (Boolean(boundUserId) && boundUserId === message.actor.providerUserId);
      if (!isBoundSender) {
        groupHistory.append(bot.id, message.actor.chatId ?? "", {
          ...(message.actor.displayName ? { senderName: message.actor.displayName } : {}),
          text: message.text,
        });
        return { ok: false, reply: [] };
      }
      // 群白名单已下线：群聊默认不限制任何群——服务谁由「私聊方式」与「群聊方式」共同决定。
      // mention 模式：未被 @ 的消息静默忽略（仅记入上下文，供下次被 @ 时参考）。
      if (activation === "mention" && message.actor.isMention !== true) {
        groupHistory.append(bot.id, message.actor.chatId ?? "", {
          ...(message.actor.displayName ? { senderName: message.actor.displayName } : {}),
          text: message.text,
        });
        return { ok: false, reply: [] };
      }
    }
    // 私聊方式=全部用户：不要求绑定，任何私聊用户都能驱动（命令权限按 bot 自身配置）。
    const user = privateChatAllUsers ? bot : findBoundUser(bot, message.actor);
    if (!user) {
      return {
        ok: false,
        reply: [createOutbound(message.actor, msg(locale, "userNotBound"))],
      };
    }
    if (!isUserCommandAllowed(user, requestedCommand)) {
      return {
        ok: false,
        reply: [createOutbound(message.actor, msg(locale, "commandNotAllowed"))],
      };
    }
    const context = await readContext(message.actor, bot);
    if (!context) {
      return {
        ok: false,
        reply: [createOutbound(message.actor, msg(locale, "noWorkspaceAllowed"))],
      };
    }
    // 新入站携带的新鲜 context_token 先落库，再用它补投停放的回复，
    // 保证补投发生在本条消息的业务处理之前（docs/specs/bot-outbound-delivery.md 规则 6）。
    const deliveryContext = message.actor.providerContextToken
      ? await refreshDeliveryInbound(bot, context, message.actor.providerContextToken)
      : context;
    // 全部用户模式没有绑定用户：主动消息（心跳/镜像）的投递目标按对话表推导
    // （最近活跃的私聊对话），不再单独记 lastPrivateUserId。
    const synced = await normalizeBotWorkspaceConfig(config, bot, {
      id: context.workspaceId ?? getWorkspaceKey(context.workspacePath, context.workspaceIdentity),
      label: getWorkspaceLabel(context.workspacePath),
      workspacePath: context.workspacePath,
      workspaceIdentity: context.workspaceIdentity,
    });
    if (
      context.workspaceId &&
      !isWorkspaceAllowed(context.workspaceId, synced.user.allowedWorkspaces)
    ) {
      // 绑定即授权：绑定工作区即使不在 allowedWorkspaces 里也必须放行。
      // 否则用户在工作区 B 打开面板把访问范围改成"已选 B"后，绑定在 A 的 bot
      // 会在每条消息上被误判越权（且提示的 /项目 切换同样被拒绝，形成死循环）。
      const boundWorkspaces = await resolveBotBoundWorkspaces(bot.id);
      const isBoundWorkspace = boundWorkspaces.some((ref) => ref.id === context.workspaceId);
      if (!isBoundWorkspace) {
        return {
          ok: false,
          reply: [createOutbound(message.actor, msg(locale, "workspaceOutOfScope"))],
        };
      }
    }
    const remoteDisconnectedReply = await blockDisconnectedRemoteWorkspace({
      message,
      context,
      locale,
      requestedCommand,
    });
    if (remoteDisconnectedReply) {
      return { ok: false, reply: remoteDisconnectedReply };
    }
    if (isFeishuBotProvider(bot.provider)) {
      // Bugfix: 飞书短命令 typing 现在由同步回复完成后显式删除。
      // 这里必须等 reaction 创建完成，否则 stopInboundTyping 可能先执行，最终留下无法清理的 Typing reaction。
      await sendTyping(bot, message.actor);
    } else {
      void sendTyping(bot, message.actor);
    }
    return {
      ok: true,
      config: synced.config,
      bot: synced.bot,
      user: synced.user,
      context: deliveryContext,
      locale,
    };
  }

  async function handleBind(
    message: BotInboundMessage,
    code: string,
  ): Promise<BotOutboundMessage[]> {
    const locale = await readMessageLocale();
    if (message.actor.chatType !== "private") {
      return [createOutbound(message.actor, msg(locale, "bindPrivateOnly"))];
    }
    const record = bindCodes.get(code.trim().toUpperCase());
    if (!record || record.expiresAt <= Date.now() || record.botId !== message.botId) {
      return [createOutbound(message.actor, msg(locale, "bindCodeInvalid"))];
    }
    const config = await repo.readConfig();
    const bot = findBot(config, record.botId);
    if (!bot) {
      return [createOutbound(message.actor, msg(locale, "bindBotMissing"))];
    }
    // Bot 配置化后 /bind 只绑定当前 bot，不再向 bot 追加 allowedUsers。
    // 重新绑定会覆盖旧 providerUserId，保证一个 bot 同一时间只有一个沟通对象。
    const nextBot: BotConfig = {
      ...bot,
      providerUserId: message.actor.providerUserId,
      displayName: message.actor.displayName,
      allowedWorkspaces: normalizeAllowedWorkspaces(record.allowedWorkspaces),
      allowedCommands: normalizeBotCommandPolicy(bot.allowedCommands),
      replyMode: normalizeBotReplyGranularity(bot.provider, bot.replyMode),
    };
    validateBotConfig(config, nextBot);
    await repo.writeConfig({
      ...config,
      bots: config.bots.map((item) => (item.id === nextBot.id ? nextBot : item)),
    });
    bindCodes.delete(record.code);
    // 绑定成功不再自动回复"绑定成功 + 命令清单"：绑定是用户主动发起的动作，
    // 后续命令清单可由 **/帮助** 主动索取（这里保持静默，避免每条绑定都刷一屏帮助）。
    return [];
  }

  /** `/send <path> | caption`：把受信目录内的文件作为媒体发到当前会话。 */
  async function handleSendMedia(
    message: BotInboundMessage,
    value: string,
  ): Promise<BotOutboundMessage[]> {
    const auth = await withAuthorizedContext(message, "message");
    if (!auth.ok) {
      return auth.reply;
    }
    const separator = value.indexOf("|");
    const filePath = (separator >= 0 ? value.slice(0, separator) : value).trim();
    const caption = separator >= 0 ? value.slice(separator + 1).trim() : "";
    if (!filePath) {
      return [createOutbound(message.actor, msg(auth.locale, "sendMediaMissing"))];
    }
    try {
      await deliverBotMedia({
        bot: auth.bot,
        filePath,
        ...(caption ? { caption } : {}),
        // 群聊回复到群；私聊默认发给绑定用户（deliverBotMedia 内部兜底）。
        ...(message.actor.chatType === "group" && message.actor.chatId
          ? { chatId: message.actor.chatId }
          : {}),
      });
      return [
        createOutbound(
          message.actor,
          msg(auth.locale, "sendMediaSent", { file: basename(filePath) }),
        ),
      ];
    } catch (error) {
      return [
        createOutbound(
          message.actor,
          msg(auth.locale, "sendMediaFailed", {
            message: formatUserFacingBotError(error, auth.locale),
          }),
        ),
      ];
    }
  }

  async function handleStatus(message: BotInboundMessage): Promise<BotOutboundMessage[]> {
    const auth = await withAuthorizedContext(message, "status");
    if (!auth.ok) {
      return auth.reply;
    }
    return [
      createOutbound(message.actor, await buildStatusText(auth.context, auth.locale), undefined, {
        locale: auth.locale,
      }),
    ];
  }

  async function createStatusReply(
    actor: BotActor,
    context: BotContextState,
    locale: Locale | undefined,
  ): Promise<BotOutboundMessage[]> {
    return [
      createOutbound(actor, await buildStatusText(context, locale), undefined, {
        locale,
      }),
    ];
  }

  function formatStatusLine(
    locale: Locale | undefined,
    labelId: BotMessageId,
    value: string,
  ): string {
    // Bugfix: /status 文案由服务层拼接，标签和值都要跟随 bot 当前 locale。
    return `${msg(locale, labelId)}: ${value}`;
  }

  function formatStatusStateValue(locale: Locale | undefined, state: string): string {
    if (locale === "en-US") {
      return state;
    }
    switch (state) {
      case "draft":
        return msg(locale, "statusDraft");
      case "remote disconnected":
        return msg(locale, "statusRemoteDisconnected");
      case "running":
        return msg(locale, "streamingStatusRunning");
      case "completed":
        return msg(locale, "streamingStatusCompleted");
      case "error":
      case "failed":
        return msg(locale, "streamingStatusFailed");
      case "cancelled":
        return msg(locale, "statusCancelled");
      case "stopped":
        return msg(locale, "statusStopped");
      default:
        return state;
    }
  }

  async function buildStatusText(
    context: BotContextState,
    locale: Locale | undefined,
  ): Promise<string> {
    const workspace = (await listWorkspaceRefs()).find((item) => item.id === context.workspaceId);
    if (!(await isRemoteWorkspaceConnected(context))) {
      const draftOptions = context.draftOptions;
      return [
        formatStatusLine(locale, "statusWorkspace", workspace?.label ?? context.workspacePath),
        formatStatusLine(
          locale,
          "statusModel",
          await formatStatusModelLabel(
            formatBotModelSelectionValue(draftOptions?.modelSelection),
            context,
          ),
        ),
        "------",
        formatStatusLine(locale, "statusTask", context.activeTaskId ?? msg(locale, "statusDraft")),
        formatStatusLine(
          locale,
          "statusState",
          formatStatusStateValue(locale, "remote disconnected"),
        ),
        msg(locale, "remoteDisconnectedStatus", {
          workspacePath: context.workspacePath,
        }),
      ].join("\n");
    }
    const modeTaskService = await resolveModeTaskServiceForContext(context);
    const tasks = await modeTaskService.listTasks({
      workspacePath: context.workspacePath,
      workspaceIdentity: context.workspaceIdentity,
    });
    const activeTask = context.activeTaskId
      ? tasks.find((task) => task.taskId === context.activeTaskId)
      : null;
    const activeTaskSnapshot = context.activeTaskId
      ? await modeTaskService
          .getTaskSnapshot({
            taskId: context.activeTaskId,
            workspacePath: context.workspacePath,
            workspaceIdentity: context.workspaceIdentity,
          })
          .catch(() => null)
      : null;
    // Bugfix: activeTaskId 来自 bot context，不应依赖 listTasks 必然返回同一条任务。
    // 某些筛选/索引时序下 listTasks 找不到 active task，之前会跳过 snapshot，导致 Progress 永远缺失。
    const statusTask = activeTask ?? activeTaskSnapshot?.meta ?? null;
    // Bugfix: 任务结束后 /status 只保留最终状态，避免把最后一次工具/思考进度误看成仍在执行。
    const latestProgress =
      context.activeTaskId && (!statusTask || taskStatus(statusTask) === "running")
        ? (liveStatusProgressByTaskId.get(context.activeTaskId)?.text ??
          readLatestTaskProgress(activeTaskSnapshot))
        : null;
    const workedDurationMs = statusTask
      ? readTaskWorkedDurationMs(activeTaskSnapshot, statusTask)
      : null;
    const activeTaskConfigOptions = context.activeTaskId
      ? await listActiveTaskConfigOptions(context, context.activeTaskId)
      : [];
    const isDraftStatus = context.mode === "draft" || !context.activeTaskId;
    const draftOptions = !statusTask && isDraftStatus ? await ensureDraftOptions(context) : null;
    // Bot Draft 属于 Select：未显式固定模型时只展示目标 Host 当前首选，不把默认值写回配置。
    const draftView = draftOptions
      ? await readModelSelectionView(context, draftOptions.modelSelection)
      : null;
    const draftEffectiveSelection = draftOptions?.modelSelection
      ? draftView?.effectiveSelection
      : draftView?.preferredSelection;
    const statusModel =
      readConfigSelectCurrentValue(activeTaskConfigOptions, "model") ??
      statusTask?.model ??
      formatBotModelSelectionValue(draftEffectiveSelection ?? undefined) ??
      "-";
    const statusModelLabel = await formatStatusModelLabel(statusModel, context);
    return (
      [
        formatStatusLine(locale, "statusWorkspace", workspace?.label ?? context.workspacePath),
        // Bugfix: active task 显示真实 task 状态；草稿态显示 draftOptions。
        // /new 后草稿继承自当前 task，继续显示 "-" 会让用户误以为继承失败。
        formatStatusLine(locale, "statusModel", statusModelLabel),
        "------",
        statusTask
          ? formatStatusTaskLine(statusTask, msg(locale, "statusTask"))
          : formatStatusLine(locale, "statusTask", msg(locale, "statusDraft")),
        formatStatusLine(
          locale,
          "statusState",
          formatStatusStateValue(locale, statusTask ? taskStatus(statusTask) : "draft"),
        ),
        workedDurationMs !== null
          ? formatStatusLine(locale, "statusWorked", formatTaskRunningDuration(workedDurationMs))
          : null,
        latestProgress ? formatStatusLine(locale, "statusProgress", latestProgress) : null,
      ].filter(Boolean) as string[]
    ).join("\n");
  }

  async function handleHelp(message: BotInboundMessage): Promise<BotOutboundMessage[]> {
    const auth = await withAuthorizedContext(message, "help");
    if (!auth.ok) {
      return auth.reply;
    }
    return [createOutbound(message.actor, buildHelpText(auth.locale, auth.bot))];
  }

  function buildHelpText(
    locale: Locale | undefined,
    bot: Pick<BotConfig, "allowedCommands">,
  ): string {
    const lines = [msg(locale, "helpTitle")];
    for (const command of BOT_MENU_COMMAND_ORDER) {
      if (command === "help" || command === "bind") {
        lines.push(msg(locale, helpMessageByCommand[command]));
        continue;
      }
      if (bot.allowedCommands[command] === false) {
        continue;
      }
      lines.push(msg(locale, helpMessageByCommand[command]));
    }
    // /send 走消息权限（恒允许），不属于策略菜单，单独追加。
    lines.push(msg(locale, "helpSend"));
    return lines.join("\n");
  }

  function sendPromptInBackground(
    bot: BotConfig,
    actor: BotActor,
    context: BotContextState,
    taskId: string,
    traceId: string,
    content: string,
    attachments: ModePromptAttachment[],
    botDeliveryTarget?: ModeAutomationBotDeliveryTarget,
    modelSelection?: ModelSelection,
    toolDenylist?: readonly string[],
  ): void {
    // Bugfix: Telegram polling 是单循环顺序处理 update。如果这里 await session/prompt，
    // 权限按钮 callback 会一直排队到整轮任务结束，导致用户点 inline keyboard 没反应。
    // 因此 prompt 必须后台跑，polling loop 才能继续接收 /permission 回调。
    void resolveModeTaskServiceForContext(context)
      .then((modeTaskService) =>
        modeTaskService.sendPrompt({
          taskId,
          traceId,
          content,
          attachments: attachments.length > 0 ? attachments : undefined,
          botDeliveryTarget,
          modelSelection,
          ...(toolDenylist && toolDenylist.length > 0
            ? { toolDenylist: [...toolDenylist] }
            : {}),
        }),
      )
      .catch(async (error) => {
        const message = error instanceof Error ? error.message : String(error);
        const locale = await readMessageLocale();
        const userFacingMessage = formatUserFacingBotError(error, locale);
        runningTasks.delete(taskId);
        stopTyping(taskId, bot.id);
        await broadcastTaskListChange(context, taskId, "error", {
          error: message,
        });
        await sendOutbound(
          bot,
          createOutbound(
            actor,
            isSessionExpiredError(error)
              ? userFacingMessage
              : msg(locale, "taskFailed", { message: userFacingMessage }),
          ),
        ).catch(() => undefined);
      });
  }

  async function handleMessage(
    message: BotInboundMessage,
    options: { heartbeat?: boolean } = {},
  ): Promise<BotOutboundMessage[]> {
    const auth = await withAuthorizedContext(message, "message");
    if (!auth.ok) {
      return auth.reply;
    }
    let deletedTaskId: string | undefined;
    if (auth.context.mode === "task" && auth.context.activeTaskId) {
      const taskService = await resolveModeTaskServiceForContext(auth.context);
      const deletedTaskIds = await taskService.listDeletedTaskIds({
        workspacePath: auth.context.workspacePath,
        workspaceIdentity: auth.context.workspaceIdentity,
      });
      if (deletedTaskIds.includes(auth.context.activeTaskId)) {
        // 桌面软删除只留下 tombstone，CLI 仍可恢复旧 session。
        // Bot 不能只凭 activeTaskId 续跑隐藏任务；先清旧交互，再复用当前草稿有效选择和 V4 首发。
        // 仅以删除记录为准，不能把列表过滤、归档或查询失败当成删除。
        deletedTaskId = auth.context.activeTaskId;
        auth.context = await writeDraftContext(auth.context);
      }
    }
    const elicitationReply = await handlePendingElicitationText(auth, message.actor, message.text);
    if (elicitationReply) {
      return elicitationReply;
    }
    if (
      auth.context.mode === "task" &&
      auth.context.activeTaskId &&
      (await isContextActiveTaskRunning(auth.context))
    ) {
      // 心跳回合遇到运行中的任务直接跳过：它不是用户消息，不能进排队队列。
      if (options.heartbeat) {
        return [];
      }
      // 任务运行中：纯文本消息入队，任务终态后自动按序投递。
      // 带附件的消息不排队（附件下载链接是瞬时的，重投递时会失效），仍请用户稍后重发。
      if (message.attachments && message.attachments.length > 0) {
        return [createOutbound(message.actor, msg(auth.locale, "taskRunning"))];
      }
      const queued = await queueContextMessage(auth.context, message);
      const replies = [
        createOutbound(
          message.actor,
          msg(auth.locale, "taskQueued", { position: String(queued.position) }),
        ),
      ];
      if (queued.dropped) {
        replies.push(createOutbound(message.actor, msg(auth.locale, "taskQueuedDropped")));
      }
      return replies;
    }
    let preparedMessage: PreparedBotMessageContent;
    try {
      preparedMessage = await prepareBotMessageContent(auth.bot, message, auth.locale);
    } catch (error) {
      return [
        createOutbound(
          message.actor,
          msg(auth.locale, "attachmentRejected", {
            message: formatAttachmentRejectedReason(error, auth.locale),
          }),
        ),
      ];
    }
    // 群聊被触发：注入群身份/回复规则 + 未参与期间的历史 + 带发送者与 @ 标记的当前消息。
    // 「全部消息」模式下模型可用 <NO_REPLY> 保持沉默（回复侧收口见 watchTaskStream）。
    const isGroupTurn = message.actor.chatType === "group";
    const groupActivation = isGroupTurn
      ? (auth.bot.groupChat?.activation ?? "disabled")
      : "disabled";
    if (isGroupTurn && message.actor.chatId) {
      const historyContext = formatGroupHistoryContext(
        groupHistory.drain(auth.bot.id, message.actor.chatId),
        auth.locale,
      );
      const groupPromptInput: GroupTurnPromptInput = {
        ...(auth.bot.name?.trim() ? { botName: auth.bot.name.trim() } : {}),
        activation: groupActivation === "always" ? "always" : "mention",
        // 无法识别 @ 的平台（微信/Webhook）按"定向消息"处理：mention 判定缺失时不静默丢消息。
        isMention: message.actor.isMention !== false,
        ...(message.actor.displayName?.trim()
          ? { senderName: message.actor.displayName.trim() }
          : {}),
        chatId: message.actor.chatId,
        providerLabel: formatBotProviderLabel(message.actor.provider),
        receivedAt: message.receivedAt ?? Date.now(),
        locale: auth.locale,
        historyContext,
        content: preparedMessage.content,
      };
      preparedMessage = {
        ...preparedMessage,
        content: buildGroupTurnPrompt(groupPromptInput),
      };
    }
    if (auth.context.mode === "draft" || !auth.context.activeTaskId) {
      const draftOptions =
        auth.context.draftOptions ?? (await buildInitializedDraftOptions(auth.context));
      // 原因：直接提交旧账号身份会绕过统一解析。只在首次创建前解析原意图；
      // 后续创建、配置和首发固定这份结果；绑定后以 Session 原选择解析下一次新输入。
      const selectionView = await readModelSelectionView(auth.context, draftOptions.modelSelection);
      const submissionModelSelection = draftOptions.modelSelection
        ? selectionView?.effectiveSelection
        : selectionView?.preferredSelection;
      if (
        !submissionModelSelection ||
        (draftOptions.modelSelection && selectionView?.selectionIssue)
      ) {
        throw new Error("Bot 无法从目标 Host 解析 Submission 模型");
      }
      const submissionDraftOptions: BotDraftOptions = {
        ...draftOptions,
        modelSelection: {
          providerId: submissionModelSelection.providerId,
          modelId: submissionModelSelection.modelId,
          ...(submissionModelSelection.options
            ? { options: { ...submissionModelSelection.options } }
            : {}),
        },
      };
      const modeTaskService = await resolveModeTaskServiceForContext(auth.context);
      const task = await modeTaskService.createTask({
        workspacePath: auth.context.workspacePath,
        workspaceIdentity: auth.context.workspaceIdentity,
        provider: draftOptions.provider,
        modelSelection: submissionDraftOptions.modelSelection,
        // 修复原因：Bot 旧 createTask 走 legacy session/create，却紧接着用 v4 sendText，
        // 内存标志与 v4 draft 持久化边界不一致，session_input 会触发 FK。改为先创建
        // v4 draft，再沿既有能力校验应用配置，最后通过 v4 sendText 首发。
        v4Create: true,
      });
      // 心跳回合不写会话：新建任务时也不用心跳提示词当标题，
      // 否则侧栏会冒出"[心跳] 主动检查…"这种任务名。
      const taskTitle = options.heartbeat
        ? msg(auth.locale, "heartbeatTaskTitle")
        : deriveTaskTitle(preparedMessage.content, preparedMessage.modeAttachments);
      const broadcastTask = taskTitle ? { ...task, title: taskTitle } : task;
      const traceId = generateTraceId(task.taskId);
      try {
        await applyDraftConfigOptions(
          { ...auth.context, draftOptions: submissionDraftOptions },
          task.taskId,
          traceId,
        );
      } catch (error) {
        // Bugfix: 初始配置失败时旧流程已把 context 切到 task，留下无法继续的空任务。
        // 在持久化 Bot task 状态前完成配置，并删除临时 task，让用户修正配置后可以直接重试。
        await modeTaskService
          .deleteTask({
            taskId: task.taskId,
            workspacePath: auth.context.workspacePath,
            workspaceIdentity: auth.context.workspaceIdentity,
          })
          .catch(() => undefined);
        throw error;
      }
      const context = {
        ...auth.context,
        mode: "task" as const,
        activeTaskId: task.taskId,
        draftOptions: undefined,
      };
      await writeContext(context);
      // Bugfix: Bot 首发不经过 UI 本地 deriveTaskTitle/optimistic cache。
      // 如果 created 广播继续携带 createTask 的空标题，侧栏会一直显示 New task，直到整表刷新。
      await broadcastTaskListChange(context, task.taskId, "created", {
        task: broadcastTask,
      });
      // bot 侧新建了会话：通知 UI 跟随（消费端只在目标工作区 tab 已打开时跳转）。
      await broadcastTaskListChange(context, task.taskId, "active_task_changed", {
        source: "bot",
      });
      if (deletedTaskId) {
        botsLogger.info(
          undefined,
          `replaced deleted Bot task bot=${auth.bot.id} oldTask=${deletedTaskId} newTask=${task.taskId} workspace=${getWorkspaceKey(context.workspacePath, context.workspaceIdentity)}`,
        );
        // 切换已经持久化；通知失败不能让 callback 释放去重记录并重跑原消息。
        await sendOutbound(
          auth.bot,
          createOutbound(message.actor, msg(auth.locale, "deletedTaskReplaced")),
        ).catch((error: unknown) => {
          botsLogger.warn(
            undefined,
            `deleted task replacement notice failed bot=${auth.bot.id} task=${task.taskId}: ${error instanceof Error ? error.message : String(error)}`,
          );
        });
      }
      runningTasks.add(task.taskId);
      await watchTaskStream(auth.bot, message.actor, context, auth.user, options.heartbeat === true);
      await broadcastTaskListChange(context, task.taskId, "prompt_sent", {
        task: broadcastTask,
        prompt: {
          content: preparedMessage.content,
          attachments:
            preparedMessage.modeAttachments.length > 0
              ? preparedMessage.modeAttachments
              : undefined,
          messageId: `bot-${traceId}`,
          sentAt: Date.now(),
        },
      });
      sendPromptInBackground(
        auth.bot,
        message.actor,
        context,
        task.taskId,
        traceId,
        preparedMessage.content,
        preparedMessage.modeAttachments,
        resolveAutomationBotDeliveryTarget(message.actor),
        submissionDraftOptions.modelSelection,
        isGroupTurn ? GROUP_CHAT_TOOL_DENYLIST : undefined,
      );
      return [];
    }
    const modeTaskService = await resolveModeTaskServiceForContext(auth.context);
    await modeTaskService.resumeTask({
      taskId: auth.context.activeTaskId,
      workspacePath: auth.context.workspacePath,
      workspaceIdentity: auth.context.workspaceIdentity,
    });
    // Bot 只是同一 Session 的输入端。菜单可能过滤无效值，不能拿它反推原选择，
    // 更不能重新套用 Bot 创建默认值。解析只确定本次输入，不在此改写 Session。
    const originalSelection = await modeTaskService.getTaskModelSelection({
      taskId: auth.context.activeTaskId,
    });
    const selectionView = originalSelection
      ? await readModelSelectionView(auth.context, originalSelection)
      : null;
    const effectiveSelection = selectionView?.effectiveSelection;
    if (!effectiveSelection || selectionView?.selectionIssue) {
      throw new Error(msg(auth.locale, "sessionModelUnavailable"));
    }
    await broadcastTaskListChange(auth.context, auth.context.activeTaskId, "resumed");
    runningTasks.add(auth.context.activeTaskId);
    await watchTaskStream(auth.bot, message.actor, auth.context, auth.user, options.heartbeat === true);
    const traceId = generateTraceId(auth.context.activeTaskId);
    await broadcastTaskListChange(auth.context, auth.context.activeTaskId, "prompt_sent", {
      prompt: {
        content: preparedMessage.content,
        attachments:
          preparedMessage.modeAttachments.length > 0
            ? preparedMessage.modeAttachments
            : undefined,
        messageId: `bot-${traceId}`,
        sentAt: Date.now(),
      },
    });
    sendPromptInBackground(
      auth.bot,
      message.actor,
      auth.context,
      auth.context.activeTaskId,
      traceId,
      preparedMessage.content,
      preparedMessage.modeAttachments,
      resolveAutomationBotDeliveryTarget(message.actor),
      effectiveSelection,
      isGroupTurn ? GROUP_CHAT_TOOL_DENYLIST : undefined,
    );
    return [];
  }

  async function handleTaskList(message: BotInboundMessage): Promise<BotOutboundMessage[]> {
    const auth = await withAuthorizedContext(message, "task");
    if (!auth.ok) {
      return auth.reply;
    }
    if (await isContextActiveTaskRunning(auth.context)) {
      // Bugfix: 运行中展示 /task 列表会让用户继续点选其它 task，
      // 即使后续切换被拒绝，也会留下误导性的 pending selection。
      return [createOutbound(message.actor, msg(auth.locale, "taskRunning"))];
    }
    const taskEntries = (await listContextTaskSelectionEntries(auth.context, auth.user)).slice(
      0,
      10,
    );
    const tasks = taskEntries.map((entry) => entry.task);
    const activeTask = auth.context.activeTaskId
      ? tasks.find((task) => task.taskId === auth.context.activeTaskId)
      : null;
    const selection: SelectionPrompt = {
      id: `task-${Date.now()}`,
      title: msg(auth.locale, "taskSelectTitle", {
        task: activeTask ? `${activeTask.title} (${activeTask.taskId})` : "draft",
      }),
      currentId: auth.context.activeTaskId ?? undefined,
      action: "task.set",
      options: taskEntries.map((entry) => ({
        id: entry.task.taskId,
        label: entry.task.title,
        description: taskStatus(entry.task),
      })),
    };
    if (taskEntries.length > 0) {
      // Bugfix: 远端 task 展示时必须把 workspaceIdentity 一起缓存。
      // 否则点击 /task 的序号后只剩 taskId，后续二次查询会退回 path-only 语义并提示 Task not found。
      pendingTaskSelectionsByContext.set(
        getActorContextKey(message.actor),
        new Map(taskEntries.map((entry) => [entry.task.taskId, entry])),
      );
    } else {
      pendingTaskSelectionsByContext.delete(getActorContextKey(message.actor));
    }
    return tasks.length > 0
      ? createSelectionReply(message.actor, selection, auth.locale)
      : [createOutbound(message.actor, msg(auth.locale, "noHistoryTasks"))];
  }

  async function resolveTaskSelectionEntry(
    message: BotInboundMessage,
    context: BotContextState,
    user: BotConfig,
    value: string,
  ): Promise<BotTaskSelectionEntry | null> {
    const pendingEntry = resolvePendingTaskSelectionEntry(message.actor, value);
    if (pendingEntry) {
      return pendingEntry;
    }
    const entries = await listContextTaskSelectionEntries(context, user);
    const selected = resolveOptionByValue(
      entries.map((entry) => ({
        id: entry.task.taskId,
        label: entry.task.title,
        entry,
      })),
      value,
    );
    if (selected) {
      return selected.entry;
    }
    const modeTaskService = await resolveModeTaskServiceForContext(context);
    const snapshot = await modeTaskService
      .getTaskSnapshot({
        taskId: value.trim(),
        workspacePath: context.workspacePath,
        workspaceIdentity: context.workspaceIdentity,
      })
      .catch(() => null);
    return snapshot
      ? {
          task: snapshot.meta,
          workspacePath: context.workspacePath,
          workspaceIdentity: context.workspaceIdentity,
        }
      : null;
  }

  async function isContextActiveTaskRunning(context: BotContextState): Promise<boolean> {
    if (!context.activeTaskId) {
      return false;
    }
    if (!runningTasks.has(context.activeTaskId)) {
      return false;
    }
    if (context.workspaceIdentity && !(await isRemoteWorkspaceConnected(context))) {
      // Bugfix: /workspace 这类本地命令只是在切换上下文，不能为了确认旧任务状态而创建远端 runtime。
      // 断连时把内存 running 状态视为不可确认，交给显式 /reconnect 后再恢复查询。
      return false;
    }
    const modeTaskService = await resolveModeTaskServiceForContext(context);
    const activeTaskSnapshot = await modeTaskService
      .getTaskSnapshot({
        taskId: context.activeTaskId,
        workspacePath: context.workspacePath,
        workspaceIdentity: context.workspaceIdentity,
      })
      .catch(() => null);
    if (
      activeTaskSnapshot?.meta.status === "completed" ||
      activeTaskSnapshot?.meta.status === "error"
    ) {
      // Bugfix: Bots 进程内 runningTasks 可能因重启/流式终态事件丢失而和持久化状态不一致。
      // Mode Agent 历史任务的 status 为空也可能只是旧数据，不代表 UI 仍在运行；只有本进程确实发起
      // 且尚未观察到终态的 task 才阻止 /task、/new 等上下文切换。
      runningTasks.delete(context.activeTaskId);
      stopTyping(context.activeTaskId, context.botId);
      return false;
    }
    return true;
  }

  function warnAutomationDeliveryOnce(params: {
    target: ModeAutomationBotDeliveryTarget;
    reason: string;
  }): void {
    const key = `${params.target.provider}:${params.target.botId}:${params.reason}`;
    const now = Date.now();
    const previousAt = automationDeliveryWarningAtByKey.get(key) ?? 0;
    if (now - previousAt < BOT_AUTOMATION_DELIVERY_WARNING_TTL_MS) return;
    automationDeliveryWarningAtByKey.set(key, now);
    botsLogger.warn(
      undefined,
      `automation Bot delivery skipped provider=${params.target.provider} bot=${params.target.botId} reason=${params.reason}`,
    );
  }

  async function watchAutomationRun(params: BotAutomationRunWatchParams): Promise<void> {
    const config = await repo.readConfig();
    const bot = findBot(config, params.target.botId);
    if (!bot) {
      warnAutomationDeliveryOnce({ target: params.target, reason: "bot_missing" });
      return;
    }
    if (!bot.enabled) {
      warnAutomationDeliveryOnce({ target: params.target, reason: "bot_disabled" });
      return;
    }
    if (bot.provider !== params.target.provider) {
      warnAutomationDeliveryOnce({ target: params.target, reason: "provider_mismatch" });
      return;
    }
    const actor: BotActor = {
      provider: params.target.provider,
      botId: params.target.botId,
      providerUserId: params.target.providerUserId,
      chatType: params.target.chatType,
    };
    const automationConversation = getBotActorConversation(actor);
    const context: BotContextState = {
      botId: bot.id,
      conversationKey: automationConversation.key,
      conversationKind: automationConversation.kind,
      conversationId: automationConversation.id,
      workspacePath: params.workspacePath,
      ...(params.workspaceIdentity ? { workspaceIdentity: params.workspaceIdentity } : {}),
      mode: "task",
      activeTaskId: params.taskId,
      updatedAt: Date.now(),
    };
    // Automation 回推固定为终态摘要；不能复用用户当前 replyMode，否则 streaming/card
    // 会在后台任务执行过程中向原会话持续发送中间过程。
    await watchTaskStream(bot, actor, context, {
      ...bot,
      replyMode: "summary_changes",
    });
  }

  service = {
    async syncAppRuntimePreferences(preferences) {
      await deps.remoteWorkspaceService?.syncAppRuntimePreferences?.(preferences);
    },
    async getStatus() {
      const config = await repo.readConfig();
      const state = await repo.readState();
      return {
        botsCount: config.bots.length,
        enabledBotsCount: config.bots.filter((bot) => bot.enabled).length,
        // contexts 现在是"对话上下文"总数（一个 bot 可以有多个对话各自一个会话）。
        contextsCount: Object.values(state.bots).reduce(
          (total, channel) => total + Object.keys(channel.conversations).length,
          0,
        ),
        botRuntime: config.bots.map((bot) => {
          const runtime = runtimeByBotId.get(bot.id);
          return (
            runtime ?? {
              botId: bot.id,
              provider: bot.provider,
              status: bot.enabled ? "idle" : "disabled",
              message: bot.enabled ? "Bot is configured." : "Bot is disabled.",
              offset: state.bots[bot.id]?.telegramOffset,
              ...(state.bots[bot.id]?.weixinActivatedAt !== undefined
                ? { weixinActivatedAt: state.bots[bot.id]!.weixinActivatedAt }
                : {}),
            }
          );
        }),
      };
    },
    getConfig: async () => {
      // 绑定菜单按 identity 判资格，读配置前先把存量 path-only key 升级掉。
      await ensureBotWorkspaceKeysUpgraded();
      return repo.readConfig();
    },
    listWorkspaceRefs,
    getUserConfigOptions: listUserConfigOptions,
    beginFeishuRegistration(params) {
      return beginFeishuAppRegistration(params?.domain);
    },
    pollFeishuRegistration(params) {
      return pollFeishuAppRegistration(params);
    },
    beginWeixinRegistration() {
      return beginWeixinQrRegistration();
    },
    pollWeixinRegistration(params) {
      return pollWeixinQrRegistration(params);
    },
    beginWecomRegistration() {
      return beginWeComQrRegistration();
    },
    pollWecomRegistration(params) {
      return pollWeComQrRegistration(params);
    },
    async saveConfig(config) {
      const savedConfig = await repo.writeConfig(normalizeConfigBots(config));
      clearCandidateCaches();
      telegramRuntime.scheduleRefresh(savedConfig);
      weixinRuntime.scheduleRefresh(savedConfig);
      feishuRuntime.scheduleRefresh(savedConfig);
      wecomRuntime.scheduleRefresh(savedConfig);
      dingtalkRuntime.scheduleRefresh(savedConfig);
      return savedConfig;
    },
    async listBots() {
      return (await repo.readConfig()).bots;
    },
    async saveBot(params: BotSaveBotParams) {
      const config = await repo.readConfig();
      let bot: BotConfig = {
        ...params.bot,
        id: params.bot.id.trim(),
        name: params.bot.name.trim(),
        allowedWorkspaces: normalizeAllowedWorkspaces(params.bot.allowedWorkspaces),
        allowedCommands: normalizeBotCommandPolicy(params.bot.allowedCommands),
        currentOptions: normalizeBotCurrentOptions(params.bot.currentOptions),
        replyMode: normalizeBotReplyGranularity(params.bot.provider, params.bot.replyMode),
      };
      if (params.credentialValue?.trim()) {
        const key = buildBotCredentialKey(bot.id);
        await deps.credentialService.save(key, params.credentialValue.trim());
        bot = { ...bot, credentialRef: key };
      }
      if (params.webhookSecretValue?.trim()) {
        const key = buildBotWebhookSecretKey(bot.id);
        await deps.credentialService.save(key, params.webhookSecretValue.trim());
        bot = { ...bot, webhookSecretRef: key };
      }
      if (params.credentialValue?.trim() || !bot.name.trim()) {
        const adapter = providers[bot.provider];
        const resolveRetryDelaysMs = isFeishuBotProvider(bot.provider) ? [0, 800, 1_800] : [0];
        let resolvedName: string | null | undefined = null;
        let lastResolveNameError: unknown;
        for (const retryDelayMs of resolveRetryDelaysMs) {
          if (retryDelayMs > 0) {
            // Bugfix: 飞书 / Lark 扫码创建应用后，应用信息接口可能短暂不可读；重试后再回填 Bot 名称。
            await delay(retryDelayMs);
          }
          try {
            resolvedName = await adapter?.resolveName?.(bot);
            if (resolvedName?.trim()) {
              break;
            }
          } catch (error) {
            lastResolveNameError = error;
          }
        }
        if (resolvedName?.trim()) {
          bot = { ...bot, name: resolvedName.trim() };
        } else if (lastResolveNameError) {
          botsLogger.warn(
            undefined,
            `resolve bot name failed bot=${bot.id}: ${lastResolveNameError instanceof Error ? lastResolveNameError.message : String(lastResolveNameError)}`,
          );
        }
      }
      bot = normalizeBotConfig(bot);
      validateBotConfig(config, bot);
      const bots = config.bots.filter((item) => item.id !== bot.id);
      bots.push(bot);
      const savedConfig = await repo.writeConfig({ ...config, bots });
      clearCandidateCaches();
      telegramRuntime.scheduleRefresh(savedConfig);
      weixinRuntime.scheduleRefresh(savedConfig);
      feishuRuntime.scheduleRefresh(savedConfig);
      wecomRuntime.scheduleRefresh(savedConfig);
      dingtalkRuntime.scheduleRefresh(savedConfig);
      return bot;
    },
    async removeBotSecret(botId: string) {
      const config = await repo.readConfig();
      const bot = findBot(config, botId);
      if (!bot) {
        throw new Error(`Bot not found: ${botId}`);
      }
      if (bot.provider === "telegram") {
        void telegramRuntime.syncCommands({ ...bot, enabled: false });
      }
      if (isFeishuBotProvider(bot.provider)) {
        feishuRuntime.stopWebSocket(bot.id);
      }
      if (bot.provider === "weixin") {
        weixinRuntime.stopPolling(bot.id);
      }
      // Bugfix: 只移除密钥时如果保留旧绑定身份，UI 会显示“已连通”，但运行时已经没有 token 可用。
      // 这里同步清理绑定状态，让 Bot token 行回到可重新添加的状态。
      const nextBot = normalizeBotConfig({
        ...bot,
        credentialRef: undefined,
        webhookSecretRef: undefined,
        providerUserId: undefined,
        displayName: undefined,
        feishuAppId: isFeishuBotProvider(bot.provider) ? undefined : bot.feishuAppId,
      });
      const savedConfig = await repo.writeConfig({
        ...config,
        bots: config.bots.map((item) => (item.id === bot.id ? nextBot : item)),
      });
      // 锁内原子 RMW（docs/specs/bot-state-ownership.md 规则 1）。
      await repo.mutateState((state) => {
        delete state.bots[bot.id];
      });
      clearCandidateCaches();
      telegramRuntime.scheduleRefresh(savedConfig);
      weixinRuntime.scheduleRefresh(savedConfig);
      feishuRuntime.scheduleRefresh(savedConfig);
      wecomRuntime.scheduleRefresh(savedConfig);
      dingtalkRuntime.scheduleRefresh(savedConfig);
      if (bot.credentialRef) {
        await deps.credentialService.delete(bot.credentialRef);
      }
      if (bot.webhookSecretRef) {
        await deps.credentialService.delete(bot.webhookSecretRef);
      }
      return nextBot;
    },
    async deleteBot(botId: string) {
      const config = await repo.readConfig();
      const bot = findBot(config, botId);
      if (bot?.provider === "telegram") {
        void telegramRuntime.syncCommands({ ...bot, enabled: false });
      }
      if (bot && isFeishuBotProvider(bot.provider)) {
        feishuRuntime.stopWebSocket(bot.id);
      }
      if (bot?.provider === "weixin") {
        weixinRuntime.stopPolling(bot.id);
      }
      if (bot?.provider === "wecom") {
        wecomRuntime.stopConnection(bot.id);
      }
      if (bot?.provider === "dingtalk") {
        dingtalkRuntime.stopConnection(bot.id);
      }
      await repo.writeConfig({
        ...config,
        bots: config.bots.filter((item) => item.id !== botId),
      });
      clearCandidateCaches();
      telegramRuntime.scheduleRefresh();
      weixinRuntime.scheduleRefresh();
      wecomRuntime.scheduleRefresh();
      dingtalkRuntime.scheduleRefresh();
      // 锁内原子 RMW；removedConversations 在 mutator 内捕获，dispose 在锁外
      // （docs/specs/bot-state-ownership.md 规则 1/3）。
      let removedConversations: BotConversationState[] = [];
      await repo.mutateState((state) => {
        removedConversations = listConversations(state, botId);
        delete state.bots[botId];
      });
      // 删除即解绑：会话归属与工作区绑定都随 bot 一起消失，运行期订阅一并撤掉。
      // 否则绑定表里会留下无法解析的"幽灵机器人"（UI 只能显示裸 id，且会挡住空态引导）。
      disposeBotRuntimeBindings(botId);
      await pruneBotFromWorkspaceBindings(botId);
      for (const removedContext of removedConversations) {
        if (!removedContext.activeTaskId) {
          continue;
        }
        // 该对话刚失去绑定：广播一次让任务行绿点/标题栏通道图标/右键菜单立刻刷新（source ui 不反向跳转）。
        await broadcastTaskListChange(
          removedContext,
          removedContext.activeTaskId,
          "active_task_changed",
          { source: "ui" },
        ).catch(() => undefined);
      }
      if (bot?.credentialRef) {
        await deps.credentialService.delete(bot.credentialRef);
      }
      if (bot?.webhookSecretRef) {
        await deps.credentialService.delete(bot.webhookSecretRef);
      }
    },
    async testBot(botId: string): Promise<BotTestResult> {
      const config = await repo.readConfig();
      const bot = findBot(config, botId);
      if (!bot) {
        return { ok: false, message: "Bot not found." };
      }
      const adapter = providers[bot.provider];
      if (!adapter) {
        return {
          ok: false,
          message: `${bot.provider} is reserved for a future version.`,
          provider: bot.provider,
        };
      }
      return { ...(await adapter.test(bot)), provider: bot.provider };
    },
    async createBindCode(params: BotCreateBindCodeParams): Promise<BotBindCodeResult> {
      const config = await repo.readConfig();
      const botId = params.botId ?? params.botId;
      if (!botId) {
        throw new Error("Bot id is required.");
      }
      const bot = findBot(config, botId);
      if (!bot) {
        throw new Error(`Bot not found: ${botId}`);
      }
      const code = createCode();
      const expiresAt = Date.now() + (params.ttlMs ?? BOT_BIND_CODE_TTL_MS);
      const allowedWorkspaces = normalizeAllowedWorkspaces(
        params.allowedWorkspaces ?? [ALL_BOT_WORKSPACES],
      );
      bindCodes.set(code, {
        botId: botId,
        code,
        allowedWorkspaces,
        expiresAt,
      });
      return { code, expiresAt };
    },
    async bindBotToWorkspace(params: BotWorkspaceBindingParams): Promise<void> {
      const config = await repo.readConfig();
      const bot = findBot(config, params.botId);
      if (!bot) {
        throw new Error(`Bot not found: ${params.botId}`);
      }
      const workspaceKey = getWorkspaceKey(params.workspacePath, params.workspaceIdentity);
      const bindings = await readBotBindings();
      // 一个工作区可绑定多个 bot，但一个 bot 只能绑一个工作区：先把它从其他工作区摘掉再追加。
      const nextBindings = setWorkspaceBinding(
        removeBotFromOtherWorkspaces(bindings, workspaceKey, bot.id),
        workspaceKey,
        bot.id,
      );
      await writeBotBindings(nextBindings);
      // 绑定即授权：绑定时访问范围收敛为该 bot 的全部绑定工作区（不再是"原范围 + 新工作区"）。
      // 否则"工作区访问范围"与"工作区绑定"会演化成两个互相矛盾的授权来源：
      // 已绑定的 bot 仍可能因访问范围不含绑定工作区而在每条消息上被判越权。
      await convergeBotAllowedWorkspaces(bot, nextBindings);
      // 绑定动作本身视作一次 UI 工作区焦点：立即把该 bot 仍处于草稿的对话钉到该工作区
      // （已有会话绑定的对话不动，避免绑定工作区时把正在进行的对话从会话上摘掉）。
      await focusBotDraftConversationsOnWorkspace(
        bot.id,
        params.workspacePath,
        params.workspaceIdentity,
      );
    },
    async unbindBotFromWorkspace(params: BotWorkspaceBindingParams): Promise<void> {
      const workspaceKey = getWorkspaceKey(params.workspacePath, params.workspaceIdentity);
      const bindings = await readBotBindings();
      // 只摘掉这个 bot；该工作区的其他 bot 绑定不受影响。
      const nextBindings = removeWorkspaceBinding(bindings, workspaceKey, params.botId);
      await writeBotBindings(nextBindings);
      // 仍有剩余绑定时继续收敛；全部解绑后保持现状——不自动放开为"所有工作区"，
      // 避免解绑动作悄悄扩大授权（用户可在 UI 里显式调整）。
      const config = await repo.readConfig();
      const bot = findBot(config, params.botId);
      if (bot) {
        await convergeBotAllowedWorkspaces(bot, nextBindings);
      }
    },
    async listBotWorkspaceBindings(params: { botId: string }): Promise<BotWorkspaceRef[]> {
      const bindings = await readBotBindings();
      const boundKeys = Object.entries(bindings)
        .filter(([, botIds]) => botIds.includes(params.botId))
        .map(([workspaceKey]) => workspaceKey);
      if (boundKeys.length === 0) {
        return [];
      }
      const refs = await listWorkspaceRefs();
      const byId = new Map(refs.map((ref) => [ref.id, ref]));
      // 绑定 key 可能指向当前不在列表里的工作区（已关闭/归档）：仍如实返回，
      // 用 key 本身构造显示信息，避免 UI 显示"未绑定"而服务端按已绑定处理。
      return boundKeys.map((workspaceKey) => byId.get(workspaceKey) ?? createWorkspaceRef(workspaceKey));
    },
    async getWorkspaceBotBinding(
      params: Omit<BotWorkspaceBindingParams, "botId">,
    ): Promise<BotWorkspaceBindingInfo> {
      const bindings = await readBotBindings();
      return {
        botIds: getWorkspaceBoundBots(
          bindings,
          getWorkspaceKey(params.workspacePath, params.workspaceIdentity),
        ),
      };
    },
    async notifyUiSessionFocus(params: BotUiFocusParams): Promise<void> {
      await applyUiFocus(params);
    },
    /**
     * 该 bot 的候选对话：已存在的上下文 + 绑定用户的私聊。
     * 绑定菜单据此让用户选择"把哪个 IM 对话绑到这个会话"。
     */
    async listBotConversations(params: { botId: string }): Promise<
      Array<{
        botId: string;
        conversationKey: string;
        conversationKind: BotConversationKind;
        conversationId: string;
        conversationLabel?: string;
        taskId: string | null;
        workspacePath?: string;
      }>
    > {
      const [config, state] = await Promise.all([repo.readConfig(), repo.readState()]);
      const bot = findBot(config, params.botId);
      if (!bot) {
        return [];
      }
      const result: Array<{
        botId: string;
        conversationKey: string;
        conversationKind: BotConversationKind;
        conversationId: string;
        conversationLabel?: string;
        taskId: string | null;
        workspacePath?: string;
      }> = [];
      const seen = new Set<string>();
      const push = (conversation: {
        conversationKey: string;
        conversationKind: BotConversationKind;
        conversationId: string;
        conversationLabel?: string;
        taskId?: string | null;
        workspacePath?: string;
      }): void => {
        if (seen.has(conversation.conversationKey)) {
          return;
        }
        seen.add(conversation.conversationKey);
        result.push({
          botId: params.botId,
          conversationKey: conversation.conversationKey,
          conversationKind: conversation.conversationKind,
          conversationId: conversation.conversationId,
          ...(conversation.conversationLabel
            ? { conversationLabel: conversation.conversationLabel }
            : {}),
          taskId: conversation.taskId ?? null,
          ...(conversation.workspacePath ? { workspacePath: conversation.workspacePath } : {}),
        });
      };
      // 1) 已有上下文的对话（按最近更新排序）
      for (const context of listConversations(state, params.botId)) {
        push({
          conversationKey: context.conversationKey,
          conversationKind: context.conversationKind,
          conversationId: context.conversationId,
          ...(context.conversationLabel ? { conversationLabel: context.conversationLabel } : {}),
          taskId: context.mode === "task" ? context.activeTaskId : null,
          workspacePath: context.workspacePath,
        });
      }
      // 2) 还没说过话但可预判的对话：绑定用户的私聊
      const boundUserId = bot.providerUserId?.trim();
      if (boundUserId) {
        push({
          conversationKey: makeBotConversationKey("private", boundUserId),
          conversationKind: "private",
          conversationId: boundUserId,
        });
      }
      return result;
    },

    async listBotTaskBindings(): Promise<
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
    > {
      const [config, state] = await Promise.all([repo.readConfig(), repo.readState()]);
      const result: Array<{
        botId: string;
        provider: BotProvider;
        conversationKey: string;
        conversationKind: BotConversationKind;
        conversationId: string;
        conversationLabel?: string;
        workspacePath: string;
        workspaceIdentity?: string;
        taskId: string;
      }> = [];
      for (const [botId, channel] of Object.entries(state.bots)) {
        const bot = findBot(config, botId);
        if (!bot || !bot.enabled) {
          continue;
        }
        for (const context of Object.values(channel.conversations)) {
          if (context.mode !== "task" || !context.activeTaskId) {
            continue;
          }
          result.push({
            botId,
            provider: bot.provider,
            conversationKey: context.conversationKey,
            conversationKind: context.conversationKind,
            conversationId: context.conversationId,
            ...(context.conversationLabel
              ? { conversationLabel: context.conversationLabel }
              : {}),
            workspacePath: context.workspacePath,
            ...(context.workspaceIdentity
              ? { workspaceIdentity: context.workspaceIdentity }
              : {}),
            taskId: context.activeTaskId,
          });
        }
      }
      return result;
    },

    /** 把机器人绑定到指定桌面会话（对齐 /task set 的语义；桌面右键菜单入口）。 */
    async bindBotToTask(params: {
      botId: string;
      /** 绑定哪个 IM 对话到该会话；省略时用该 bot 当前唯一/最近活跃的对话。 */
      conversationKey?: string;
      workspacePath: string;
      workspaceIdentity?: string;
      taskId: string;
    }): Promise<{ ok: boolean; reason?: "busy" | "missing" | "workspace" | "conversation" }> {
      // 资格判定（绑定表 + allowedWorkspaces）都按 identity key 计算：
      // 先跑一次性升级，避免升级前读到的 path-only 授权让首次绑定被误判 workspace。
      await ensureBotWorkspaceKeysUpgraded();
      const config = await repo.readConfig();
      const bot = findBot(config, params.botId);
      if (!bot) {
        return { ok: false, reason: "missing" };
      }
      const workspaceKey = getWorkspaceKey(params.workspacePath, params.workspaceIdentity);
      // 会话只能绑定属于当前工作区的机器人（工作区绑定的那些，或显式授权本工作区的 bot）。
      // 通配范围的全局 bot 不算归属：否则任意 bot 都能绑进任意工作区，这条约束形同虚设。
      const botBindings = await readBotBindings();
      if (
        !isBotEligibleForSessionBinding({
          bot,
          workspaceKey,
          workspaceBoundBotIds: getWorkspaceBoundBots(botBindings, workspaceKey),
        })
      ) {
        return { ok: false, reason: "workspace" };
      }
      // 绑定到任务同样隐含工作区访问（与工作区绑定一致：绑定即授权）。
      // 走到这里 bot 必然已授权本工作区或正是工作区绑定的 bot，这一步只兜底收敛失败的历史数据。
      if (!isWorkspaceAllowed(workspaceKey, bot.allowedWorkspaces)) {
        await service.saveBot({
          bot: {
            ...bot,
            allowedWorkspaces: normalizeAllowedWorkspaces([...bot.allowedWorkspaces, workspaceKey]),
          },
        });
      }
      const state = await repo.readState();
      const conversations = listConversations(state, params.botId);
      const requestedKey = params.conversationKey?.trim();
      if (requestedKey && !parseBotConversationKey(requestedKey)) {
        return { ok: false, reason: "conversation" };
      }
      // 未指定对话时：优先该 bot 已有对话里最近活跃的那个；还没有任何对话时按配置推导默认对话
      // （绑定用户的私聊）——与 listBotConversations 的候选口径一致，桌面菜单总能拿到一个对话键。
      const defaultKey = (() => {
        const existing = conversations[0]?.conversationKey;
        if (existing) {
          return existing;
        }
        const boundUserId = bot.providerUserId?.trim();
        return boundUserId ? makeBotConversationKey("private", boundUserId) : undefined;
      })();
      const targetKey = requestedKey ?? defaultKey;
      if (!targetKey) {
        return { ok: false, reason: "conversation" };
      }
      const targetContext = readConversation(state, params.botId, targetKey);
      if (
        targetContext?.activeTaskId &&
        targetContext.activeTaskId !== params.taskId &&
        (await isContextActiveTaskRunning(targetContext))
      ) {
        // 该对话正在别的任务上工作：此时换绑会让回复串线，交给用户先 /stop 或等结束。
        return { ok: false, reason: "busy" };
      }
      // 一个会话可以同时绑定多个机器人（同一工作区的其他 bot 不受影响）：各自镜像到自己的渠道。
      // 同一个 bot 的其他对话也不受影响：各对话各持一个会话，互不打断。
      await focusBotOnWorkspace(
        params.botId,
        targetKey,
        params.workspacePath,
        params.workspaceIdentity,
        params.taskId,
      );
      return { ok: true };
    },

    /**
     * 解除机器人与任务的绑定：回到草稿并停止向该会话继续投递。
     * 指定 conversationKey 时只解绑该对话；省略时解绑该 bot 所有指向此任务的对话。
     */
    async unbindBotFromTask(params: {
      botId: string;
      taskId: string;
      conversationKey?: string;
    }): Promise<void> {
      const state = await repo.readState();
      const requestedKey = params.conversationKey?.trim();
      const targets = listConversationsByTask(state, params.botId, params.taskId).filter(
        (conversation) => !requestedKey || conversation.conversationKey === requestedKey,
      );
      for (const context of targets) {
        // 回到草稿：writeContext 会撤掉该会话上这个 bot 的流订阅（避免解绑后正文继续推到 IM），
        // key 含 botId + 对话键，同一会话的其他机器人与该 bot 的其他对话订阅都不受影响。
        await writeDraftContext(context, undefined);
      }
    },

    /**
     * 桌面输入镜像（对齐 MyAgents 的 im-mirror）：绑定会话里由桌面端发出的 prompt
     * 转发到 IM 会话，与 IM→桌面方向合起来构成双向实时同步。
     * 由渲染层在用户主动发送时调用（天然排除机器人自身产生的回合，不会回声）。
     */
    /**
     * 桌面端发送前的镜像武装：给绑定到该 task 的每个对话挂上助手回复流订阅。
     * 与提问回显分开，是因为订阅必须早于提示进入 Agent（晚一轮就整轮收不到事件）。
     */
    async armConversationReplyMirror(params: {
      taskId: string;
      workspacePath: string;
      workspaceIdentity?: string;
    }): Promise<void> {
      const state = await repo.readState();
      const config = await repo.readConfig();
      const workspaceKey = getWorkspaceKey(params.workspacePath, params.workspaceIdentity);
      for (const [botId, channel] of Object.entries(state.bots)) {
        const bot = findBot(config, botId);
        if (!bot || !bot.enabled) {
          continue;
        }
        for (const context of Object.values(channel.conversations)) {
          if (
            context.mode !== "task" ||
            context.activeTaskId !== params.taskId ||
            getWorkspaceKey(context.workspacePath, context.workspaceIdentity) !== workspaceKey
          ) {
            continue;
          }
          await ensureContextStreamWatch(bot, context);
        }
      }
    },

    async notifyDesktopUserMessage(params: {
      taskId: string;
      workspacePath: string;
      workspaceIdentity?: string;
      text: string;
    }): Promise<void> {
      const text = params.text.trim();
      if (!text) {
        return;
      }
      // 幂等复用同一条武装逻辑，覆盖「回显先于武装」的调用顺序。
      await this.armConversationReplyMirror(params);
      const state = await repo.readState();
      const config = await repo.readConfig();
      const workspaceKey = getWorkspaceKey(params.workspacePath, params.workspaceIdentity);
      const locale = await readMessageLocale();
      for (const [botId, channel] of Object.entries(state.bots)) {
        const bot = findBot(config, botId);
        if (!bot || !bot.enabled) {
          continue;
        }
        for (const context of Object.values(channel.conversations)) {
          if (
            context.mode !== "task" ||
            context.activeTaskId !== params.taskId ||
            getWorkspaceKey(context.workspacePath, context.workspaceIdentity) !== workspaceKey
          ) {
            continue;
          }
          // 谁持有这个会话就往谁的对话镜像：群聊回群、私聊回本人。
          const mirrorUserId =
            context.conversationKind === "private"
              ? context.conversationId
              : resolveBotProactiveUserId(bot, context, state);
          if (!mirrorUserId) {
            continue;
          }
          const adapter = providers[bot.provider];
          if (!adapter) {
            continue;
          }
          try {
            await adapter.send(bot, {
              botId: bot.id,
              provider: bot.provider,
              providerUserId: mirrorUserId,
              text: formatBotMessage(locale, "desktopPromptMirror", { text }),
            });
          } catch (error) {
            botsLogger.warn(
              undefined,
              `desktop prompt mirror failed bot=${botId}: ${
                error instanceof Error ? error.message : String(error)
              }`,
            );
          }
        }
      }
    },

    async sendBotMedia(params: BotSendMediaParams): Promise<void> {
      const config = await repo.readConfig();
      const bot = findBot(config, params.botId);
      if (!bot) {
        throw new Error(`Bot not found: ${params.botId}`);
      }
      await deliverBotMedia({
        bot,
        filePath: params.filePath,
        ...(params.caption ? { caption: params.caption } : {}),
        ...(params.chatId ? { chatId: params.chatId } : {}),
      });
    },
    /** 所有对话上下文（展平）：UI 用它显示每个 bot 当前挂着的会话/工作区。 */
    async getBotStates() {
      const state = await repo.readState();
      return Object.values(state.bots).flatMap((channel) =>
        Object.values(channel.conversations),
      );
    },
    /**
     * 重置状态：键是对话键时只清该对话；是 botId 时清该 bot 的全部对话
     * （保留通道级游标，避免重置后 Telegram 重投旧更新）。
     */
    async resetBotState(contextKey: string) {
      const parsed = parseBotConversationKey(contextKey);
      // 锁内原子 RMW（docs/specs/bot-state-ownership.md 规则 1）。
      await repo.mutateState((state) => {
        if (parsed) {
          for (const channel of Object.values(state.bots)) {
            if (channel.conversations[contextKey]) {
              delete channel.conversations[contextKey];
              channel.updatedAt = Date.now();
            }
          }
        } else if (state.bots[contextKey]) {
          state.bots[contextKey].conversations = {};
          state.bots[contextKey].updatedAt = Date.now();
        }
      });
    },
    watchAutomationRun,
    async handleInboundMessage(message: BotInboundMessage) {
      return enqueueInboundProcessing(message.actor, async () => {
        if (message.elicitationResponse) {
          return handleStructuredElicitationResponse(message, message.elicitationResponse);
        }
        const parsedCommand = parseBotCommand(message.text);
        const command =
          parsedCommand.type === "message"
            ? (resolvePendingSelectionCommand(message.actor, parsedCommand.text) ?? parsedCommand)
            : parsedCommand.type === "selection.cancel" &&
                message.actor.provider !== "weixin" &&
                message.text.trim() === "0"
              ? (clearPendingSelection(message.actor),
                { type: "message", text: message.text } as const)
              : parsedCommand;
        const weixinActivationReply = await handleWeixinFirstActivation(message, command);
        if (weixinActivationReply) {
          return weixinActivationReply;
        }
        if (
          message.actor.chatType === "group" &&
          command.type !== "message" &&
          command.type !== "help" &&
          command.type !== "stop" &&
          command.type !== "selection.cancel" &&
          command.type !== "approve" &&
          command.type !== "deny"
        ) {
          // 群聊只开对话类能力：管理命令（/model、/task 等）在群里会改变所有成员的会话上下文。
          return [
            createOutbound(
              message.actor,
              msg(await readMessageLocale(), "groupCommandUnsupported"),
            ),
          ];
        }
        switch (command.type) {
          case "selection.cancel":
            return handleSelectionCancel(message);
          case "bind":
            return handleBind(message, command.code);
          case "help":
            return handleHelp(message);
          case "status":
            return handleStatus(message);
          case "reconnect":
            return handleReconnect(message);
          case "new": {
            const auth = await withAuthorizedContext(message, "new");
            if (!auth.ok) return auth.reply;
            if (await isContextActiveTaskRunning(auth.context)) {
              return [createOutbound(message.actor, msg(auth.locale, "taskRunning"))];
            }
            const context = await writeDraftContext(
              auth.context,
              await buildActiveTaskDraftOptions(auth.context),
            );
            // /new 开新会话：该群的旧历史不再作为上下文（否则会把上一话题带进新会话）。
            if (message.actor.chatType === "group" && message.actor.chatId) {
              groupHistory.clear(message.botId, message.actor.chatId);
            }
            return createStatusReply(message.actor, context, auth.locale);
          }
          case "workspace.list": {
            const auth = await withAuthorizedContext(message, "workspace");
            if (!auth.ok) return auth.reply;
            if (await isContextActiveTaskRunning(auth.context)) {
              // Bugfix: task 运行中不展示 workspace 选择，避免用户误以为可以切换上下文。
              return [createOutbound(message.actor, msg(auth.locale, "taskRunning"))];
            }
            const synced = await normalizeBotWorkspaceConfig(
              auth.config,
              auth.bot,
              createCurrentWorkspaceRef(auth.context),
            );
            // 绑定的 bot 菜单只展示绑定集：与 /workspace.set 的接受范围保持一致，
            // 避免菜单列出会被拒绝（或按序号落到另一个工作区）的选项。
            const boundWorkspaces = await resolveBotBoundWorkspaces(auth.bot.id);
            const visibleWorkspaces =
              boundWorkspaces.length > 0
                ? boundWorkspaces
                : filterAllowedWorkspaces(synced.workspaces, synced.user.allowedWorkspaces);
            const options = visibleWorkspaces.map((workspace) => ({
              id: workspace.id,
              // Bugfix: Telegram/飞书等按钮通道只展示 label，不展示 description。
              // 远端标识必须合进 label，避免 /workspace 列表看不出哪些项目来自远端。
              label: formatWorkspaceOptionLabel(workspace, auth.locale),
            }));
            if (options.length === 0) {
              pendingWorkspaceSelectionsByContext.delete(getActorContextKey(message.actor));
              return [createOutbound(message.actor, msg(auth.locale, "workspaceMissing"))];
            }
            // Bugfix: 远端 workspace 选项必须在展示时保留 workspaceIdentity。
            // Telegram/飞书按钮会把点击变成 /workspace 序号，切换阶段若重新从 settings 解析，
            // current remote context 可能不在候选列表里，最终表现成 /workspace 不支持远端。
            pendingWorkspaceSelectionsByContext.set(
              getActorContextKey(message.actor),
              new Map(visibleWorkspaces.map((workspace) => [workspace.id, { workspace }])),
            );
            return createSelectionReply(
              message.actor,
              {
                id: `workspace-${Date.now()}`,
                title: msg(auth.locale, "workspaceSelectTitle", {
                  workspace:
                    visibleWorkspaces.find((workspace) => workspace.id === auth.context.workspaceId)
                      ?.label ?? auth.context.workspacePath,
                }),
                currentId: auth.context.workspaceId,
                action: "workspace.set",
                options,
              },
              auth.locale,
            );
          }
          case "workspace.set": {
            const auth = await withAuthorizedContext(message, "workspace");
            if (!auth.ok) return auth.reply;
            if (await isContextActiveTaskRunning(auth.context)) {
              return [createOutbound(message.actor, msg(auth.locale, "taskRunning"))];
            }
            // 绑定的 bot 由工作区侧管理：上下文钉定在绑定集内，IM 侧 /workspace 不能跳出。
            // 绑定集只有一个元素时显式拒绝；多绑定工作区仍可在绑定集内切换。
            const boundWorkspaces = await resolveBotBoundWorkspaces(auth.bot.id);
            const selectedPinned =
              boundWorkspaces.length > 0
                ? resolveWorkspaceByValue(
                    boundWorkspaces,
                    command.value,
                    boundWorkspaces.map((ref) => ref.id),
                  )
                : null;
            if (boundWorkspaces.length > 0) {
              if (!selectedPinned) {
                return [createOutbound(message.actor, msg(auth.locale, "workspacePinned"))];
              }
              const context = {
                ...auth.context,
                workspacePath: selectedPinned.workspacePath,
                workspaceIdentity: selectedPinned.workspaceIdentity,
                workspaceId: selectedPinned.id,
              };
              const draftContext = await writeDraftContext(
                context,
                await buildInitializedDraftOptions(context),
              );
              pendingWorkspaceSelectionsByContext.delete(getActorContextKey(message.actor));
              return createStatusReply(message.actor, draftContext, auth.locale);
            }
            const synced = await normalizeBotWorkspaceConfig(
              auth.config,
              auth.bot,
              createCurrentWorkspaceRef(auth.context),
            );
            const workspace =
              resolvePendingWorkspaceSelectionEntry(message.actor, command.value)?.workspace ??
              resolveWorkspaceByValue(
                synced.workspaces,
                command.value,
                synced.user.allowedWorkspaces,
              );
            if (!workspace)
              return [createOutbound(message.actor, msg(auth.locale, "workspaceMissing"))];
            const context = {
              ...auth.context,
              workspacePath: workspace.workspacePath,
              workspaceIdentity: workspace.workspaceIdentity,
              workspaceId: workspace.id,
            };
            const draftContext = await writeDraftContext(
              context,
              await buildInitializedDraftOptions(context),
            );
            pendingWorkspaceSelectionsByContext.delete(getActorContextKey(message.actor));
            return createStatusReply(message.actor, draftContext, auth.locale);
          }
          case "model.list": {
            const auth = await withAuthorizedContext(message, "model");
            if (!auth.ok) return auth.reply;
            if (await isContextActiveTaskRunning(auth.context)) {
              // Bugfix: task 运行中不展示模型选择，避免产生运行中不可用的 pending selection。
              return [createOutbound(message.actor, msg(auth.locale, "taskRunning"))];
            }
            if (auth.context.mode === "draft" || !auth.context.activeTaskId) {
              const draftOptions = await resolveDraftOptionsForDisplay(auth.context);
              const providers = await listModelProviderOptionsForActiveTask(
                {
                  model: formatBotModelSelectionValue(draftOptions.modelSelection),
                  workspacePath: auth.context.workspacePath,
                  workspaceIdentity: auth.context.workspaceIdentity,
                },
                draftOptions.provider,
              );
              if (providers.length === 0) {
                return [createOutbound(message.actor, msg(auth.locale, "modelProviderMissing"))];
              }
              const currentProviderId = await readCurrentModelProviderId(
                {
                  model: formatBotModelSelectionValue(draftOptions.modelSelection),
                  workspacePath: auth.context.workspacePath,
                  workspaceIdentity: auth.context.workspaceIdentity,
                },
                [],
                draftOptions.provider,
              );
              return createSelectionReply(
                message.actor,
                {
                  id: `model-${Date.now()}`,
                  title: msg(auth.locale, "modelProviderSelectTitle", {
                    model: await formatStatusModelLabel(
                      formatBotModelSelectionValue(draftOptions.modelSelection),
                      auth.context,
                    ),
                  }),
                  currentId: currentProviderId,
                  action: "model.provider.set",
                  options: providers,
                },
                auth.locale,
              );
            }
            const active = await requireActiveTask(message, auth);
            if (!active.ok) return active.reply;
            const activeProvider = normalizeAgentProviderToModeAgent(active.task.provider);
            if (!activeProvider) {
              return [createOutbound(message.actor, msg(auth.locale, "modelProviderMissing"))];
            }
            const providers = await listModelProviderOptionsForActiveTask(
              active.task,
              activeProvider,
            );
            if (providers.length === 0) {
              return [createOutbound(message.actor, msg(auth.locale, "modelProviderMissing"))];
            }
            const currentValue = readCurrentActiveTaskModel(active.task, active.configOptions);
            const currentProviderId = await readCurrentModelProviderId(
              active.task,
              active.configOptions,
              activeProvider,
            );
            return createSelectionReply(
              message.actor,
              {
                id: `model-${Date.now()}`,
                title: msg(auth.locale, "modelProviderSelectTitle", {
                  model: await formatStatusModelLabel(currentValue, active.task),
                }),
                currentId: currentProviderId,
                action: "model.provider.set",
                options: providers,
              },
              auth.locale,
            );
          }
          case "model.provider.set": {
            const auth = await withAuthorizedContext(message, "model");
            if (!auth.ok) return auth.reply;
            if (await isContextActiveTaskRunning(auth.context)) {
              return [createOutbound(message.actor, msg(auth.locale, "taskRunning"))];
            }
            if (auth.context.mode === "draft" || !auth.context.activeTaskId) {
              const draftOptions = await resolveDraftOptionsForDisplay(auth.context);
              const draftTask = {
                model: formatBotModelSelectionValue(draftOptions.modelSelection),
                workspacePath: auth.context.workspacePath,
                workspaceIdentity: auth.context.workspaceIdentity,
              };
              const providers = await listModelProviderOptionsForActiveTask(
                draftTask,
                draftOptions.provider,
              );
              const provider =
                resolvePendingSelectionOption(message.actor, "model.provider.set", command.value) ??
                resolveOptionByValue(providers, command.value);
              const providerId = provider?.id;
              if (!providerId) {
                return [createOutbound(message.actor, msg(auth.locale, "modelProviderMissing"))];
              }
              const providerModels = readModelProviderSelectionModels(provider);
              const options =
                providerModels.length > 0
                  ? providerModels
                  : await listModelOptionsForProviderFromActiveTask(
                      draftTask,
                      draftOptions.provider,
                      providerId,
                    );
              if (options.length === 0) {
                return [createOutbound(message.actor, msg(auth.locale, "modelMissing"))];
              }
              return createSelectionReply(
                message.actor,
                {
                  id: `model-${Date.now()}`,
                  title: msg(auth.locale, "modelModelSelectTitle", {
                    model: formatBotModelSelectionValue(draftOptions.modelSelection) ?? "-",
                  }),
                  currentId: options.some(
                    (option) =>
                      option.id === formatBotModelSelectionValue(draftOptions.modelSelection),
                  )
                    ? formatBotModelSelectionValue(draftOptions.modelSelection)
                    : undefined,
                  action: "model.set",
                  options,
                },
                auth.locale,
              );
            }
            const active = await requireActiveTask(message, auth);
            if (!active.ok) return active.reply;
            const activeProvider = normalizeAgentProviderToModeAgent(active.task.provider);
            if (!activeProvider) {
              return [createOutbound(message.actor, msg(auth.locale, "modelProviderMissing"))];
            }
            const providers = await listModelProviderOptionsForActiveTask(
              active.task,
              activeProvider,
            );
            const provider =
              resolvePendingSelectionOption(message.actor, "model.provider.set", command.value) ??
              resolveOptionByValue(providers, command.value);
            const providerId = provider?.id;
            if (!providerId) {
              return [createOutbound(message.actor, msg(auth.locale, "modelProviderMissing"))];
            }
            const providerModels = readModelProviderSelectionModels(provider);
            const options =
              providerModels.length > 0
                ? providerModels
                : await listModelOptionsForProviderFromActiveTask(
                    active.task,
                    activeProvider,
                    providerId,
                  );
            if (options.length === 0) {
              return [createOutbound(message.actor, msg(auth.locale, "modelMissing"))];
            }
            const currentValue = readCurrentActiveTaskModel(active.task, active.configOptions);
            return createSelectionReply(
              message.actor,
              {
                id: `model-${Date.now()}`,
                title: msg(auth.locale, "modelModelSelectTitle", {
                  model: currentValue ?? "-",
                }),
                currentId: options.some((option) => option.id === currentValue)
                  ? currentValue
                  : undefined,
                action: "model.set",
                options,
              },
              auth.locale,
            );
          }
          case "model.set": {
            const auth = await withAuthorizedContext(message, "model");
            if (!auth.ok) return auth.reply;
            if (await isContextActiveTaskRunning(auth.context)) {
              return [createOutbound(message.actor, msg(auth.locale, "taskRunning"))];
            }
            if (auth.context.mode === "draft" || !auth.context.activeTaskId) {
              const draftOptions = await ensureDraftOptions(auth.context);
              const model =
                resolvePendingSelectionOption(message.actor, "model.set", command.value) ??
                resolveOptionByValue(
                  await listAllModelOptionsForActiveTask(
                    {
                      model: formatBotModelSelectionValue(draftOptions.modelSelection),
                      workspacePath: auth.context.workspacePath,
                      workspaceIdentity: auth.context.workspaceIdentity,
                    },
                    draftOptions.provider,
                  ),
                  command.value,
                );
              if (!model) {
                return [createOutbound(message.actor, msg(auth.locale, "modelMissing"))];
              }
              const identity = parseBotModelOptionValue(model.id);
              const view = await readModelSelectionView(auth.context);
              const selection =
                view && identity ? completeNewModelSelection(view, identity) : undefined;
              // Bot 的主动选模也须取目标最高档；旧菜单失效/读取失败不能清掉已保存选择。
              if (!selection)
                return [createOutbound(message.actor, msg(auth.locale, "modelMissing"))];
              const nextContext = await writeDraftOptions(auth.context, {
                ...draftOptions,
                // 模型身份切换必须构造全新的 Selection，不能把旧模型的显式 options 带过去。
                modelSelection: selection,
              });
              return createStatusReply(message.actor, nextContext, auth.locale);
            }
            const active = await requireActiveTask(message, auth);
            if (!active.ok) return active.reply;
            const activeProvider = normalizeAgentProviderToModeAgent(active.task.provider);
            if (!activeProvider) {
              return [createOutbound(message.actor, msg(auth.locale, "modelProviderMissing"))];
            }
            const model =
              resolvePendingSelectionOption(message.actor, "model.set", command.value) ??
              resolveOptionByValue(
                await listAllModelOptionsForActiveTask(active.task, activeProvider),
                command.value,
              );
            if (!model) return [createOutbound(message.actor, msg(auth.locale, "modelMissing"))];
            const nextModel = model.id;
            const customModel = decodeCustomModelValue(nextModel);
            const targetModel = customModel
              ? resolveCustomModelRuntimeModelId(activeProvider, customModel)
              : nextModel;
            if (!targetModel) {
              return [createOutbound(message.actor, msg(auth.locale, "modelMissing"))];
            }
            const targetIdentity = customModel?.modelName
              ? {
                  // Bugfix: bot /model 选择 custom provider 时，targetModel 会被降成纯模型名。
                  // legacy task facade 必须额外拿到原始 provider 身份，否则同名模型会退回 glm/native。
                  providerId: customModel.providerId,
                  modelId: customModel.modelName,
                }
              : { providerId: activeProvider, modelId: targetModel };
            const view = await readModelSelectionView(active.task);
            const targetModelSelection = view
              ? completeNewModelSelection(view, targetIdentity)
              : undefined;
            if (!targetModelSelection)
              return [createOutbound(message.actor, msg(auth.locale, "modelMissing"))];
            const traceId = generateTraceId(active.taskId);
            const modeTaskService = await resolveModeTaskServiceForContext(active.task);
            const configOptions = await modeTaskService.setModel({
              taskId: active.taskId,
              traceId,
              modelSelection: targetModelSelection,
            });
            await broadcastTaskConfigSync({
              context: auth.context,
              taskId: active.taskId,
              task: await readContextActiveTaskMeta(auth.context),
              provider: activeProvider,
              configOptions,
            });
            return createStatusReply(message.actor, auth.context, auth.locale);
          }
          case "mode.list":
          case "thoughtLevel.list": {
            const commandName = command.type === "mode.list" ? "mode" : "thoughtLevel";
            const auth = await withAuthorizedContext(message, commandName);
            if (!auth.ok) return auth.reply;
            if (command.type === "mode.list") {
              // Bot 硬锁 yolo：不提供模式选择。
              return [createOutbound(message.actor, msg(auth.locale, "modeLocked"))];
            }
            if (await isContextActiveTaskRunning(auth.context)) {
              // Bugfix: task 运行中不展示模式/思考级别选择，避免和正在执行的上下文配置混淆。
              return [createOutbound(message.actor, msg(auth.locale, "taskRunning"))];
            }
            if (auth.context.mode === "draft" || !auth.context.activeTaskId) {
              const draftOptions = await ensureDraftOptions(auth.context);
              const optionSource = await listDraftConfigOptions(auth.context, draftOptions);
              const rawCurrentValue =
                commandName === "mode"
                  ? draftOptions.mode
                  : findSelectConfigOption(optionSource, commandName)?.currentValue;
              const currentValue =
                typeof rawCurrentValue === "string" ? rawCurrentValue : undefined;
              const currentLabel = readConfigSelectLabelForValue(
                optionSource,
                commandName,
                currentValue,
                { locale: auth.locale, provider: draftOptions.provider },
              );
              const selectOption = findSelectConfigOption(optionSource, commandName);
              const options = listConfigSelectOptions(optionSource, commandName, {
                locale: auth.locale,
                provider: draftOptions.provider,
              });
              if (options.length === 0) {
                return [
                  createOutbound(
                    message.actor,
                    msg(auth.locale, getConfigCommandMissingMessageId(commandName)),
                  ),
                ];
              }
              return createSelectionReply(
                message.actor,
                {
                  id: `${selectOption?.id ?? commandName}-${Date.now()}`,
                  title:
                    commandName === "mode"
                      ? msg(auth.locale, "modeSelectTitle", {
                          mode: currentLabel ?? "-",
                        })
                      : msg(auth.locale, "thoughtLevelSelectTitle", {
                          level: currentLabel ?? "-",
                        }),
                  currentId: currentValue,
                  action: `${commandName}.set` as SelectionPrompt["action"],
                  options,
                },
                auth.locale,
              );
            }
            const active = await requireActiveTask(message, auth);
            if (!active.ok) return active.reply;
            const optionSource =
              commandName === "mode" && active.task.provider
                ? await listProviderConfigOptionsForActiveTask(
                    active.task,
                    normalizeAgentProviderToModeAgent(active.task.provider),
                  )
                : active.configOptions;
            const currentValue =
              commandName === "mode"
                ? readCurrentActiveTaskMode(active.task, active.configOptions)
                : readConfigSelectCurrentValue(active.configOptions, commandName);
            const currentLabel =
              commandName === "mode"
                ? readConfigSelectLabelForValue(optionSource, commandName, currentValue, {
                    locale: auth.locale,
                    provider: normalizeAgentProviderToModeAgent(active.task.provider),
                  })
                : readConfigSelectCurrentLabel(active.configOptions, commandName, {
                    locale: auth.locale,
                    provider: normalizeAgentProviderToModeAgent(active.task.provider),
                  });
            const selectOption = findSelectConfigOption(optionSource, commandName);
            const options = listConfigSelectOptions(optionSource, commandName, {
              locale: auth.locale,
              provider: normalizeAgentProviderToModeAgent(active.task.provider),
            });
            if (options.length === 0) {
              return [
                createOutbound(
                  message.actor,
                  msg(auth.locale, getConfigCommandMissingMessageId(commandName)),
                ),
              ];
            }
            return createSelectionReply(
              message.actor,
              {
                id: `${selectOption?.id ?? commandName}-${Date.now()}`,
                title:
                  commandName === "mode"
                    ? msg(auth.locale, "modeSelectTitle", {
                        mode: currentLabel ?? "-",
                      })
                    : msg(auth.locale, "thoughtLevelSelectTitle", {
                        level: currentLabel ?? "-",
                      }),
                currentId: currentValue,
                action: `${commandName}.set` as SelectionPrompt["action"],
                options,
              },
              auth.locale,
            );
          }
          case "mode.set":
          case "thoughtLevel.set": {
            const commandName = command.type === "mode.set" ? "mode" : "thoughtLevel";
            const auth = await withAuthorizedContext(message, commandName);
            if (!auth.ok) return auth.reply;
            if (command.type === "mode.set") {
              // Bot 硬锁 yolo：拒绝任何模式切换请求。
              return [createOutbound(message.actor, msg(auth.locale, "modeLocked"))];
            }
            if (await isContextActiveTaskRunning(auth.context)) {
              return [createOutbound(message.actor, msg(auth.locale, "taskRunning"))];
            }
            if (auth.context.mode === "draft" || !auth.context.activeTaskId) {
              const originalOptions = await ensureDraftOptions(auth.context);
              const view = await readModelSelectionView(
                auth.context,
                originalOptions.modelSelection,
              );
              const optionSource = await listDraftConfigOptions(
                auth.context,
                originalOptions,
                view,
              );
              // 同一个快照给出候选与当前模型；失效原意图不能因副本为空退回 preferred。
              const draftOptions = {
                ...originalOptions,
                modelSelection:
                  (originalOptions.modelSelection
                    ? view?.effectiveSelection
                    : view?.preferredSelection) ?? undefined,
              };
              const displayOptions = listConfigSelectOptions(optionSource, commandName, {
                locale: auth.locale,
                provider: draftOptions.provider,
              });
              const option =
                resolvePendingSelectionOption(
                  message.actor,
                  `${commandName}.set` as SelectionPrompt["action"],
                  command.value,
                ) ?? resolveOptionByValue(displayOptions, command.value);
              if (option && !displayOptions.some((candidate) => candidate.id === option.id)) {
                return [
                  createOutbound(
                    message.actor,
                    msg(auth.locale, getConfigCommandMissingMessageId(commandName)),
                  ),
                ];
              }
              if (!option) {
                return [
                  createOutbound(
                    message.actor,
                    msg(auth.locale, getConfigCommandMissingMessageId(commandName)),
                  ),
                ];
              }
              const nextContext = await writeDraftOptions(auth.context, {
                ...draftOptions,
                ...(commandName === "mode"
                  ? { mode: option.id }
                  : draftOptions.modelSelection
                    ? {
                        modelSelection: {
                          ...draftOptions.modelSelection,
                          options: {
                            ...draftOptions.modelSelection.options,
                            reasoningLevel: option.id,
                          },
                        },
                      }
                    : {}),
              });
              return createStatusReply(message.actor, nextContext, auth.locale);
            }
            const active = await requireActiveTask(message, auth);
            if (!active.ok) return active.reply;
            const optionSource =
              commandName === "mode" && active.task.provider
                ? await listProviderConfigOptionsForActiveTask(active.task, active.task.provider)
                : active.configOptions;
            const selectOption = findSelectConfigOption(optionSource, commandName);
            const displayOptions = listConfigSelectOptions(optionSource, commandName, {
              locale: auth.locale,
              provider: active.task.provider,
            });
            const option =
              resolvePendingSelectionOption(
                message.actor,
                `${commandName}.set` as SelectionPrompt["action"],
                command.value,
              ) ?? resolveOptionByValue(displayOptions, command.value);
            if (!option) {
              return [
                createOutbound(
                  message.actor,
                  msg(auth.locale, getConfigCommandMissingMessageId(commandName)),
                ),
              ];
            }
            if (!selectOption?.id) {
              return [
                createOutbound(
                  message.actor,
                  msg(auth.locale, getConfigCommandMissingMessageId(commandName)),
                ),
              ];
            }
            const traceId = generateTraceId(active.taskId);
            const modeTaskService = await resolveModeTaskServiceForContext(auth.context);
            const configOptions = await modeTaskService.setConfigOption({
              taskId: active.taskId,
              traceId,
              configId: selectOption.id,
              value: option.id,
            });
            await broadcastTaskConfigSync({
              context: auth.context,
              taskId: active.taskId,
              task: await readContextActiveTaskMeta(auth.context),
              provider: active.task.provider,
              configOptions,
            });
            return createStatusReply(message.actor, auth.context, auth.locale);
          }
          case "task.list":
            return handleTaskList(message);
          case "task.set": {
            const auth = await withAuthorizedContext(message, "task");
            if (!auth.ok) return auth.reply;
            const taskEntry = await resolveTaskSelectionEntry(
              message,
              auth.context,
              auth.user,
              command.value,
            );
            if (!taskEntry) return [createOutbound(message.actor, msg(auth.locale, "taskMissing"))];
            const { task } = taskEntry;
            if (
              auth.context.activeTaskId !== task.taskId &&
              (await isContextActiveTaskRunning(auth.context))
            ) {
              // Bugfix: 运行中的旧 task 已经建立了第三方 stream 订阅。
              // 如果此时允许 /task 改写 activeTaskId，后续输入会落到新 task，
              // 但旧 task 输出仍会继续回到同一 bot 会话，用户会误以为消息串线。
              return [createOutbound(message.actor, msg(auth.locale, "taskRunning"))];
            }
            const nextContext = {
              ...auth.context,
              workspacePath: taskEntry.workspacePath,
              workspaceIdentity: taskEntry.workspaceIdentity,
              workspaceId: getWorkspaceKey(taskEntry.workspacePath, taskEntry.workspaceIdentity),
              mode: "task",
              activeTaskId: task.taskId,
            } satisfies BotContextState;
            await writeContext(nextContext);
            pendingTaskSelectionsByContext.delete(getActorContextKey(message.actor));
            // bot 侧切换了目标会话：建立流观看，并通知 UI 跟随跳转。
            // UI 执行跳转后会回调 notifyUiSessionFocus，命中 focusBotOnWorkspace 的幂等短路，不会成环。
            await ensureContextStreamWatch(auth.bot, nextContext);
            await broadcastTaskListChange(nextContext, task.taskId, "active_task_changed", {
              source: "bot",
            });
            return createStatusReply(message.actor, nextContext, auth.locale);
          }
          case "reply.list": {
            const auth = await withAuthorizedContext(message, "reply");
            if (!auth.ok) return auth.reply;
            return createSelectionReply(
              message.actor,
              {
                id: `reply-${Date.now()}`,
                title: msg(auth.locale, "replySelectTitle", {
                  mode: formatReplyGranularityLabel(
                    auth.bot.replyMode,
                    auth.locale,
                    auth.bot.provider,
                  ),
                }),
                currentId: normalizeBotReplyGranularity(auth.bot.provider, auth.bot.replyMode),
                action: "reply.set",
                options: getReplyGranularityOptions(auth.locale, auth.bot.provider),
              },
              auth.locale,
            );
          }
          case "reply.set": {
            const auth = await withAuthorizedContext(message, "reply");
            if (!auth.ok) return auth.reply;
            const replyGranularity = resolveReplyGranularityByValue(
              command.value,
              auth.locale,
              auth.bot.provider,
            );
            if (!replyGranularity)
              return [createOutbound(message.actor, msg(auth.locale, "replyMissing"))];
            await service.saveBot({
              bot: {
                ...auth.bot,
                replyMode: replyGranularity.id,
              },
            });
            return createStatusReply(message.actor, auth.context, auth.locale);
          }
          case "send": {
            return handleSendMedia(message, command.value);
          }
          case "stop": {
            const auth = await withAuthorizedContext(message, "stop");
            if (!auth.ok) return auth.reply;
            if (!auth.context.activeTaskId) {
              return [createOutbound(message.actor, msg(auth.locale, "noActiveTask"))];
            }
            try {
              const modeTaskService = await resolveModeTaskServiceForContext(auth.context);
              await modeTaskService.stopGeneration({
                taskId: auth.context.activeTaskId,
              });
            } catch (error) {
              const messageText = error instanceof Error ? error.message : String(error);
              return [
                createOutbound(
                  message.actor,
                  msg(auth.locale, "taskFailed", { message: messageText }),
                ),
              ];
            }
            runningTasks.delete(auth.context.activeTaskId);
            stopTyping(auth.context.activeTaskId, auth.bot.id);
            await broadcastTaskListChange(auth.context, auth.context.activeTaskId, "updated");
            return createStatusReply(message.actor, auth.context, auth.locale);
          }
          case "permission.respond": {
            const auth = await withAuthorizedContext(message, "approve");
            if (!auth.ok) return auth.reply;
            if (!auth.context.activeTaskId)
              return [createOutbound(message.actor, msg(auth.locale, "noActiveTask"))];
            const optionIndex = Number.parseInt(command.value, 10) - 1;
            const option = Number.isFinite(optionIndex)
              ? auth.context.pendingPermissionOptions?.[optionIndex]
              : undefined;
            botsLogger.info(
              undefined,
              `permission callback task=${auth.context.activeTaskId} user=${message.actor.providerUserId} optionIndex=${optionIndex + 1} pending=${auth.context.pendingPermissionOptions?.length ?? 0} option=${option ? `${option.command}:${option.optionId}` : "missing"} handled=${option?.handledAt ? "yes" : "no"}`,
            );
            if (!option)
              return [createOutbound(message.actor, msg(auth.locale, "permissionExpired"))];
            if (option.handledAt)
              return [createOutbound(message.actor, msg(auth.locale, "permissionHandled"))];
            const modeTaskService = await resolveModeTaskServiceForContext(auth.context);
            const submitted = await modeTaskService.respondPermission({
              taskId: auth.context.activeTaskId,
              requestId: option.requestId,
              optionId: option.optionId,
              response: option.response,
            });
            // 修复原因：权限和问答必须以同一个 v4 ACK 为提交点。ACK 失败前不能持久化
            // handledAt，否则 Telegram/文本序号按钮无法重试，runtime 仍会继续等待权限。
            const handledAt = Date.now();
            const nextPermissionOptions = auth.context.pendingPermissionOptions?.map((item) =>
              item.requestId === option.requestId ? { ...item, handledAt } : item,
            );
            botsLogger.info(
              undefined,
              `permission callback respond task=${auth.context.activeTaskId} requestId=${option.requestId} optionId=${option.optionId} submitted=${submitted}`,
            );
            if (!submitted) {
              return [createOutbound(message.actor, msg(auth.locale, "permissionHandled"))];
            }
            await writeContext({
              ...auth.context,
              pendingPermissionOptions: nextPermissionOptions,
            });
            await broadcastTaskListChange(
              auth.context,
              auth.context.activeTaskId,
              "permission_resolved",
              {
                requestId: option.requestId,
              },
            );
            startTyping(auth.bot, message.actor, auth.context.activeTaskId);
            // 修复原因：权限应答成功后任务会立即恢复输出，但传输 provider 在
            // awaiting_input 时已把任务流标记为暂停。这里重新通知 started，
            // 让 provider 把当前轮次的 stream 提升为任务流并保留收口归属；
            // 否则恢复后的每帧出站都落在新建的无主 stream 上（见 providers/types.ts 的 lifecycle 说明）。
            providers[auth.bot.provider]?.notifyTaskLifecycle?.(auth.bot, message.actor, "started");
            return [
              createOutbound(
                message.actor,
                msg(
                  auth.locale,
                  option.command === "deny" ? "permissionDenied" : "permissionSubmitted",
                ),
              ),
            ];
          }
          case "elicitation.respond": {
            const auth = await withAuthorizedContext(message, "message");
            if (!auth.ok) return auth.reply;
            return handlePendingElicitationValue(auth, message.actor, command.value);
          }
          case "elicitation.submit": {
            const auth = await withAuthorizedContext(message, "message");
            if (!auth.ok) return auth.reply;
            const pending = auth.context.pendingElicitation;
            if (!pending) {
              return [createOutbound(message.actor, msg(auth.locale, "elicitationExpired"))];
            }
            if (message.actor.provider !== "weixin") {
              // Bugfix: 非微信通道的“完成”应从带 token 的按钮进入 elicitation.respond。
              // 直接 /elicitation submit 没有轮次标识，可能误提交上一轮 AskUserQuestion。
              return [createOutbound(message.actor, msg(auth.locale, "elicitationExpired"))];
            }
            return submitPendingElicitation(
              auth,
              message.actor,
              pending,
              "accept",
              buildBotElicitationContent(pending),
            );
          }
          case "approve": {
            const auth = await withAuthorizedContext(message, "approve");
            if (!auth.ok) return auth.reply;
            if (!auth.context.activeTaskId)
              return [createOutbound(message.actor, msg(auth.locale, "noActiveTask"))];
            const pendingOption = auth.context.pendingPermissionOptions?.find(
              (option) =>
                option.requestId === command.requestId && option.optionId === command.optionId,
            );
            if (!pendingOption) {
              return [createOutbound(message.actor, msg(auth.locale, "permissionHandled"))];
            }
            const modeTaskService = await resolveModeTaskServiceForContext(auth.context);
            const submitted = await modeTaskService.respondPermission({
              taskId: auth.context.activeTaskId,
              requestId: command.requestId,
              optionId: command.optionId,
              response: pendingOption.response,
            });
            if (!submitted)
              return [createOutbound(message.actor, msg(auth.locale, "permissionHandled"))];
            await broadcastTaskListChange(
              auth.context,
              auth.context.activeTaskId,
              "permission_resolved",
              {
                requestId: command.requestId,
              },
            );
            startTyping(auth.bot, message.actor, auth.context.activeTaskId);
            return [createOutbound(message.actor, msg(auth.locale, "permissionSubmitted"))];
          }
          case "deny": {
            const auth = await withAuthorizedContext(message, "approve");
            if (!auth.ok) return auth.reply;
            if (!auth.context.activeTaskId)
              return [createOutbound(message.actor, msg(auth.locale, "noActiveTask"))];
            const pendingOption = auth.context.pendingPermissionOptions?.find(
              (option) => option.requestId === command.requestId && option.command === "deny",
            );
            const modeTaskService = await resolveModeTaskServiceForContext(auth.context);
            const submitted = await modeTaskService.respondPermission({
              taskId: auth.context.activeTaskId,
              requestId: command.requestId,
              optionId: "deny",
              response: pendingOption?.response ?? {
                decision: "deny",
                reason: "Denied by bot command",
              },
            });
            if (!submitted)
              return [createOutbound(message.actor, msg(auth.locale, "permissionHandled"))];
            await broadcastTaskListChange(
              auth.context,
              auth.context.activeTaskId,
              "permission_resolved",
              {
                requestId: command.requestId,
              },
            );
            startTyping(auth.bot, message.actor, auth.context.activeTaskId);
            return [createOutbound(message.actor, msg(auth.locale, "permissionDenied"))];
          }
          case "unknown":
            return [
              createOutbound(
                message.actor,
                msg(await readMessageLocale(), "unknownCommand", {
                  command: command.name,
                }),
              ),
            ];
          case "message":
            return handleMessage(message);
        }
      });
    },
    async handleProviderCallback(provider: BotProvider, payload: unknown) {
      return (await processProviderCallback(provider, payload)).replies;
    },
    async handleProviderCallbackResponse(provider: BotProvider, payload: unknown) {
      return processProviderCallback(provider, payload);
    },
    disposeAll() {
      void service.disposeAllAndWait().catch((error: unknown) => {
        botsLogger.warn(
          undefined,
          `dispose Bot runtimes failed: ${error instanceof Error ? error.message : String(error)}`,
        );
      });
    },
    disposeAllAndWait() {
      if (shutdownPromise) {
        return shutdownPromise;
      }
      stopBotHeartbeatScheduler();
      memoryDiagnostics.dispose();
      for (const controller of streamingCardRequestControllers) {
        controller.abort(new Error("Bot service disposed."));
      }
      streamingCardRequestControllers.clear();
      for (const subscription of streamSubscriptions.values()) {
        subscription.dispose();
      }
      streamSubscriptions.clear();
      transientInteractionCards.clear();
      for (const intervalId of typingIntervals.values()) {
        clearInterval(intervalId);
      }
      typingIntervals.clear();
      for (const activeTyping of typingTargets.values()) {
        const adapter = providers[activeTyping.bot.provider];
        void adapter?.stopTyping?.(activeTyping.bot, activeTyping.target).catch(() => undefined);
      }
      typingTargets.clear();
      runningTasks.clear();
      liveStatusProgressByTaskId.clear();
      pendingRemoteReconnectsByKey.clear();
      recentRemoteReconnectAtByKey.clear();
      recentRemoteReconnectDeliveryAtByKey.clear();
      recentInboundDeliveryAtByKey.clear();
      automationDeliveryWarningAtByKey.clear();
      inboundProcessingQueuesByContext.clear();
      // Bugfix：host 的异步资源回收会优先调用 disposeAllAndWait。保留统一 Promise，确保并发关闭
      // 只执行一次，并在返回前等三类 Provider runtime 的请求、WebSocket 和跨进程锁全部收口。
      shutdownPromise = Promise.allSettled([
        telegramRuntime.dispose(),
        weixinRuntime.dispose(),
        feishuRuntime.dispose(),
        wecomRuntime.dispose(),
        dingtalkRuntime.dispose(),
      ]).then(() => undefined);
      return shutdownPromise;
    },
  };
  if (runStartupBackgroundTasks) {
    void telegramRuntime.refresh();
    void weixinRuntime.refresh();
    void feishuRuntime.refresh();
    void wecomRuntime.refresh();
    void dingtalkRuntime.refresh();
    startBotHeartbeatScheduler();
    void ensureBotStorageMigrated().catch((error: unknown) => {
      // 首次读取失败必须可见，不能产生未处理 rejection；交互入口仍直接收到该错误。
      botsLogger.error(
        undefined,
        `Bot storage initialization failed: ${error instanceof Error ? error.message : String(error)}`,
      );
    });
  }
  return service;
}
