/* eslint-disable max-lines -- 钉钉 provider 集中处理 Stream 回调解析、Markdown 发送与 token 生命周期。 */
import { randomUUID } from "node:crypto";
import type { BotConfig, BotInboundMessage } from "@zcode/shared";
import type { BotProviderAdapter, BotStreamingReplyCardState } from "./types.js";
import { splitBotText } from "../botText.js";
import { formatBotMessage } from "../messages.js";
import { renderStreamingBlocksToMarkdown } from "./streamingText.js";
import { fetchBotProviderJson } from "#src/bots/providers/providerRequest.js";

/**
 * 钉钉 provider（对齐 MyAgents 的 dingtalk 适配器）。
 *
 * 接收：Stream 模式长连接（dintalkChannelRuntime 维护），回调帧 topic
 * `/v1.0/im/bot/messages/get`，data 是字符串化 JSON。
 * 发送：机器人 OpenAPI（robotCode = clientId）：
 * - 单聊 `/v1.0/robot/oToMessages/batchSend`（userIds = staffId）
 * - 群聊 `/v1.0/robot/groupMessages/send`（openConversationId）
 * 会话 id 约定与 MyAgents 一致：群聊存 `group:{openConversationId}`，单聊存 staffId。
 *
 * 平台限制：钉钉机器人没有消息编辑/流式 API（AI 卡片除外），因此不支持
 * streaming_card 粒度——回复按 Markdown 摘要粒度发送，超长按 20000 字符分片。
 */

export const DINGTALK_API_BASE = "https://api.dingtalk.com";
/** 钉钉单条消息上限（字符）；与 MyAgents 的 MAX_MESSAGE_LENGTH 一致。 */
export const DINGTALK_MESSAGE_LIMIT = 20_000;
const DINGTALK_MARKDOWN_TITLE = "AI 助手";
/** token 提前刷新余量（秒）。 */
const TOKEN_REFRESH_MARGIN_SECONDS = 300;
const TOKEN_DEFAULT_VALIDITY_SECONDS = 7_200;
/** 群聊会话 id 前缀：与 MyAgents 的 `group:{openConversationId}` 约定一致。 */
const DINGTALK_GROUP_CHAT_PREFIX = "group:";

interface DingtalkProviderDeps {
  loadCredential(key: string): Promise<string | null>;
}

interface DingtalkTokenResponse {
  accessToken?: string;
  expireIn?: number;
}

interface DingtalkApiResponse {
  processQueryKey?: string;
  code?: string;
  message?: string;
}

interface DingtalkTokenCacheEntry {
  token: string;
  expiresAt: number;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function readString(record: Record<string, unknown> | null | undefined, key: string): string {
  const value = record?.[key];
  return typeof value === "string" ? value.trim() : "";
}

/**
   * AI 卡片是否可用：必须同时开启开关并配置模板 ID（对齐 MyAgents 的门槛）。
   * 服务层用同一判定决定是否走卡片流式——未配置时回退 Markdown 摘要，
   * 避免 create 返回 null 触发失败退避把终稿一起熔断。
   */
export function isDingtalkAiCardEnabled(bot: Pick<BotConfig, "dingtalkUseAiCard" | "dingtalkCardTemplateId">): boolean {
  return bot.dingtalkUseAiCard === true && Boolean(bot.dingtalkCardTemplateId?.trim());
}

/** 群聊/单聊的发送目标判定；导出供单测。 */
export function resolveDingtalkSendTarget(chatId: string): {
  kind: "group" | "private";
  target: string;
} {
  if (chatId.startsWith(DINGTALK_GROUP_CHAT_PREFIX)) {
    return { kind: "group", target: chatId.slice(DINGTALK_GROUP_CHAT_PREFIX.length) };
  }
  return { kind: "private", target: chatId };
}

/** 解析钉钉回调 data（字符串化 JSON）里的消息；导出供单测。 */
export function parseDingtalkBotMessage(
  botId: string,
  data: Record<string, unknown>,
): BotInboundMessage | null {
  const messageId = readString(data, "msgId");
  if (!messageId) {
    return null;
  }
  const msgType = readString(data, "msgtype") || "text";
  let text = "";
  if (msgType === "text") {
    text = readString(isRecord(data.text) ? data.text : null, "content");
  } else if (msgType === "richText") {
    const richText = isRecord(data.content) && Array.isArray(data.content.richText)
      ? data.content.richText
      : [];
    text = richText
      .filter(isRecord)
      .map((item) => (typeof item.text === "string" ? item.text : ""))
      .join("");
  } else {
    text = `[不支持的消息类型: ${msgType}]`;
  }
  const trimmed = text.trim();
  if (!trimmed) {
    return null;
  }

  const senderStaffId = readString(data, "senderStaffId");
  const senderNick = readString(data, "senderNick");
  const conversationType = readString(data, "conversationType") || "1";
  const isGroup = conversationType === "2";
  const conversationId = readString(data, "conversationId");
  // isInAtList 在不同版本可能是 bool 或字符串 "true"（对齐 MyAgents 的两种取值）。
  const isInAtListRaw = data.isInAtList;
  const isInAtList =
    isInAtListRaw === true || (typeof isInAtListRaw === "string" && isInAtListRaw === "true");

  // 群聊在群内回复（providerUserId 仍保留发送者 staffId 供授权/身份使用，
  // 回复目标由 chatId 表达，createOutbound 会按 chatId 路由）。
  const chatId = isGroup ? `${DINGTALK_GROUP_CHAT_PREFIX}${conversationId}` : senderStaffId;
  return {
    botId,
    text: trimmed,
    actor: {
      provider: "dingtalk",
      botId,
      providerUserId: senderStaffId,
      ...(senderNick ? { displayName: senderNick } : {}),
      chatType: isGroup ? "group" : "private",
      ...(chatId ? { chatId } : {}),
      providerMessageId: messageId,
      // 单聊天然"被 @"；群聊按 isInAtList（钉钉在 @ 机器人时才回调）。
      isMention: isGroup ? isInAtList : true,
    },
  };
}

export function createDingtalkBotProvider(deps: DingtalkProviderDeps): BotProviderAdapter {
  const tokenCache = new Map<string, DingtalkTokenCacheEntry>();
  /** 活跃 AI 卡片：chatId → outTrackId + 最近内容（内容未变时跳过更新，对齐 MyAgents）。 */
  const activeCards = new Map<string, { outTrackId: string; lastContent: string }>();
  let tokenRefreshInFlight: Promise<string | null> | null = null;

  async function loadSecret(bot: BotConfig): Promise<string | null> {
    return bot.credentialRef ? deps.loadCredential(bot.credentialRef) : null;
  }

  async function getAccessToken(bot: BotConfig): Promise<string | null> {
    const clientId = bot.dingtalkClientId?.trim() ?? "";
    const cacheKey = `${clientId}:${bot.credentialRef ?? ""}`;
    const cached = tokenCache.get(cacheKey);
    if (cached && cached.expiresAt > Date.now()) {
      return cached.token;
    }
    const secret = await loadSecret(bot);
    if (!clientId || !secret?.trim()) {
      return null;
    }
    // 同一 bot 的并发调用共享一次刷新（对齐 MyAgents 的 mutex 双检）。
    if (!tokenRefreshInFlight) {
      tokenRefreshInFlight = (async () => {
        try {
          const response = await fetchBotProviderJson<DingtalkTokenResponse>(
            `${DINGTALK_API_BASE}/v1.0/oauth2/accessToken`,
            {
              method: "POST",
              headers: { "content-type": "application/json" },
              body: JSON.stringify({ appKey: clientId, appSecret: secret.trim() }),
            },
          );
          const token = response.payload?.accessToken;
          if (!response.ok || !token) {
            return null;
          }
          const expireIn = response.payload?.expireIn ?? TOKEN_DEFAULT_VALIDITY_SECONDS;
          tokenCache.set(cacheKey, {
            token,
            expiresAt:
              Date.now() + Math.max(60, expireIn - TOKEN_REFRESH_MARGIN_SECONDS) * 1000,
          });
          return token;
        } finally {
          tokenRefreshInFlight = null;
        }
      })();
    }
    return tokenRefreshInFlight;
  }

  async function apiCall(
    bot: BotConfig,
    path: string,
    body: Record<string, unknown>,
    method: "POST" | "PUT" = "POST",
  ): Promise<{ ok: boolean; payload: DingtalkApiResponse | undefined; status: number }> {
    let token = await getAccessToken(bot);
    if (!token) {
      throw new Error("DingTalk access token is unavailable.");
    }
    let response = await fetchBotProviderJson<DingtalkApiResponse>(
      `${DINGTALK_API_BASE}${path}`,
      {
        method,
        headers: {
          "content-type": "application/json",
          "x-acs-dingtalk-access-token": token,
        },
        body: JSON.stringify(body),
      },
    );
    if (response.status === 401) {
      // token 失效：清缓存重取一次（对齐 MyAgents 的 401 重试）。
      tokenCache.clear();
      token = await getAccessToken(bot);
      if (!token) {
        throw new Error("DingTalk access token refresh failed.");
      }
      response = await fetchBotProviderJson<DingtalkApiResponse>(
        `${DINGTALK_API_BASE}${path}`,
        {
          method: "POST",
          headers: {
            "content-type": "application/json",
            "x-acs-dingtalk-access-token": token,
          },
          body: JSON.stringify(body),
        },
      );
    }
    return { ok: response.ok, payload: response.payload, status: response.status };
  }

  async function sendMarkdown(bot: BotConfig, chatId: string, text: string): Promise<void> {
    const robotCode = bot.dingtalkClientId?.trim();
    if (!robotCode) {
      throw new Error("DingTalk clientId (robotCode) is missing.");
    }
    const target = resolveDingtalkSendTarget(chatId);
    const msgParam = JSON.stringify({ title: DINGTALK_MARKDOWN_TITLE, text });
    const result =
      target.kind === "group"
        ? await apiCall(bot, "/v1.0/robot/groupMessages/send", {
            robotCode,
            openConversationId: target.target,
            msgKey: "sampleMarkdown",
            msgParam,
          })
        : await apiCall(bot, "/v1.0/robot/oToMessages/batchSend", {
            robotCode,
            userIds: [target.target],
            msgKey: "sampleMarkdown",
            msgParam,
          });
    if (!result.ok || (result.payload?.code && result.payload.code !== "0")) {
      throw new Error(
        `DingTalk send failed: ${result.payload?.message ?? `HTTP ${result.status}`}`,
      );
    }
  }

  const isAiCardEnabled = isDingtalkAiCardEnabled;

  function renderCardText(state: BotStreamingReplyCardState): string {
    return renderStreamingBlocksToMarkdown(state, formatBotMessage(state.locale, "streamingWorking"));
  }

  /** 创建并投递 AI 卡片（createAndDeliver），返回 outTrackId 作为消息句柄。 */
  async function createAiCard(bot: BotConfig, chatId: string, initialText: string): Promise<string> {
    const templateId = bot.dingtalkCardTemplateId?.trim();
    const robotCode = bot.dingtalkClientId?.trim();
    if (!templateId || !robotCode) {
      throw new Error("DingTalk AI card requires cardTemplateId and clientId.");
    }
    const outTrackId = randomUUID();
    const target = resolveDingtalkSendTarget(chatId);
    const isGroup = target.kind === "group";
    const result = await apiCall(
      bot,
      "/v1.0/card/instances/createAndDeliver",
      {
        cardTemplateId: templateId,
        outTrackId,
        cardData: { cardParamMap: { content: initialText } },
        // openSpaceId 与投递模型按单聊/群聊区分（对齐 MyAgents 的 dtv1.card//IM_* 约定）。
        openSpaceId: isGroup
          ? `dtv1.card//IM_GROUP.${target.target}`
          : `dtv1.card//IM_ROBOT.${target.target}`,
        imGroupOpenDeliverModel: isGroup ? { robotCode } : {},
        imRobotOpenDeliverModel: isGroup ? {} : { robotCode },
        callbackType: "STREAM",
      },
    );
    if (!result.ok) {
      throw new Error(`DingTalk card create failed: HTTP ${result.status}`);
    }
    return outTrackId;
  }

  /** 卡片流式更新（isFinalize=true 收尾），对齐 MyAgents 的 /v1.0/card/streaming 载荷。 */
  async function updateAiCard(
    bot: BotConfig,
    outTrackId: string,
    content: string,
    isFinalize: boolean,
  ): Promise<void> {
    const result = await apiCall(
      bot,
      "/v1.0/card/streaming",
      {
        outTrackId,
        key: "content",
        content,
        isFull: true,
        isFinalize,
        guid: randomUUID(),
      },
      "PUT",
    );
    if (!result.ok) {
      throw new Error(`DingTalk card stream failed: HTTP ${result.status}`);
    }
  }

  return {
    async test(bot) {
      const clientId = bot.dingtalkClientId?.trim();
      const secret = await loadSecret(bot);
      if (!clientId || !secret?.trim()) {
        return { ok: false, message: "DingTalk clientId / AppSecret is missing." };
      }
      const token = await getAccessToken(bot);
      if (!token) {
        return { ok: false, message: "DingTalk access token request failed." };
      }
      return { ok: true, name: clientId, message: "DingTalk credentials are valid." };
    },

    async send(bot, message) {
      // 钉钉单条上限 20000 字符；按边界分片（复用共享分片器）。
      for (const chunk of splitBotText(message.text, DINGTALK_MESSAGE_LIMIT)) {
        await sendMarkdown(bot, message.providerUserId, chunk);
      }
    },

    // AI 卡片流式：仅在配置了模板时启用（服务层同样按此门槛判定，未配置一律走 Markdown 摘要）。
    async createStreamingReplyCard(bot, state) {
      if (!isAiCardEnabled(bot)) {
        return null;
      }
      const chatId = state.providerUserId;
      const text = renderCardText(state);
      const outTrackId = await createAiCard(bot, chatId, text);
      activeCards.set(chatId, { outTrackId, lastContent: text });
      return { providerMessageId: outTrackId };
    },

    async updateStreamingReplyCard(bot, handle, state) {
      if (!isAiCardEnabled(bot)) {
        return;
      }
      const chatId = state.providerUserId;
      const active = activeCards.get(chatId);
      const outTrackId = active?.outTrackId ?? handle.providerMessageId;
      const isFinal = state.status !== "running";
      const text = renderCardText(state);
      if (!isFinal && active && active.lastContent === text) {
        return;
      }
      await updateAiCard(bot, outTrackId, text, isFinal);
      if (isFinal) {
        activeCards.delete(chatId);
      } else if (active) {
        active.lastContent = text;
      }
    },

    parseCallback(payload): BotInboundMessage[] {
      if (!isRecord(payload)) {
        return [];
      }
      const botId = typeof payload.botId === "string" ? payload.botId : "";
      const frame = isRecord(payload.frame) ? payload.frame : null;
      const headers = frame && isRecord(frame.headers) ? frame.headers : null;
      const topic = readString(headers, "topic");
      // 只处理机器人消息回调（兼容带尾斜杠的变体，对齐 MyAgents）。
      if (topic !== "/v1.0/im/bot/messages/get" && topic !== "/v1.0/im/bot/messages/get/") {
        return [];
      }
      const data =
        frame && typeof frame.data === "string" ? (JSON.parse(frame.data) as unknown) : null;
      if (!botId || !isRecord(data)) {
        return [];
      }
      const message = parseDingtalkBotMessage(botId, data);
      return message ? [message] : [];
    },
  };
}

