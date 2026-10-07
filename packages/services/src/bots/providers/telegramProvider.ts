/* eslint-disable max-lines -- Telegram provider 集中处理 Bot API 文本、按钮、媒体解析和附件下载。 */
import type {
  BotInboundAttachment,
  BotConfig,
  BotInboundMessage,
  BotOutboundMessage,
  SelectionPrompt,
} from "@zcode/shared";
import { BOT_MENU_COMMAND_ORDER } from "../commandOrder.js";
import { splitBotText } from "../botText.js";
import type {
  BotProviderAdapter,
  BotStreamingReplyCardState,
} from "./types.js";
import type { BotProviderJsonResponse } from "#src/bots/providers/providerRequest.js";
import {
  fetchBotProvider,
  fetchBotProviderJson,
} from "#src/bots/providers/providerRequest.js";

interface TelegramProviderDeps {
  loadCredential(key: string): Promise<string | null>;
}

interface TelegramBotCommand {
  command: string;
  description: string;
}

const TELEGRAM_ATTACHMENT_DOWNLOAD_TIMEOUT_MS = 30_000;
/** editMessageText 的文本上限与 sendMessage 一致（4096 UTF-16 code units 上再留余量）。 */
const TELEGRAM_EDIT_TEXT_LIMIT = 3800;
/** 媒体上传超时：图片/文档体积可能较大，单独放宽。 */
const TELEGRAM_MEDIA_UPLOAD_TIMEOUT_MS = 60_000;

interface TelegramSendMessageResponse {
  ok?: boolean;
  description?: string;
  error_code?: number;
  parameters?: {
    retry_after?: number;
  };
  result?: {
    message_id?: number;
  };
}

/** 限流重试上限与单次最大等待：长睡眠会占住 Bot 的串行出站队列，超出按上限截断。 */
const TELEGRAM_RATE_LIMIT_MAX_RETRIES = 2;
const TELEGRAM_RATE_LIMIT_MAX_SLEEP_SECONDS = 30;

function readTelegramRetryAfter(payload: unknown): number | null {
  if (typeof payload !== "object" || payload === null) {
    return null;
  }
  const retryAfter = (payload as TelegramSendMessageResponse).parameters?.retry_after;
  return typeof retryAfter === "number" && retryAfter > 0 ? retryAfter : null;
}

/**
 * Telegram JSON 调用统一入口：处理 429 的 retry_after 重试。
 * Telegram 在超限响应里明确给出等待秒数（parameters.retry_after），照它等待后重试；
 * body 是已序列化的字符串，可安全重放。
 */
async function telegramJsonCall<T>(
  url: string,
  init: RequestInit,
): Promise<BotProviderJsonResponse<T>> {
  let response = await fetchBotProviderJson<T>(url, init);
  for (
    let attempt = 0;
    attempt < TELEGRAM_RATE_LIMIT_MAX_RETRIES && !response.ok;
    attempt += 1
  ) {
    const retryAfter = readTelegramRetryAfter(response.payload);
    if (response.status !== 429 || retryAfter === null) {
      return response;
    }
    await new Promise((resolveSleep) =>
      setTimeout(
        resolveSleep,
        Math.min(retryAfter, TELEGRAM_RATE_LIMIT_MAX_SLEEP_SECONDS) * 1000,
      ),
    );
    response = await fetchBotProviderJson<T>(url, init);
  }
  return response;
}

interface TelegramGetMeResponse {
  ok?: boolean;
  description?: string;
  result?: {
    id?: number | string;
    username?: string;
    first_name?: string;
  };
}

interface TelegramFileResponse {
  ok?: boolean;
  description?: string;
  result?: {
    file_path?: string;
    file_size?: number;
  };
}

const telegramCommandDescriptions = {
  bind: "Bind this chat",
  help: "Show help",
  status: "Show current status",
  new: "Create a new task",
  workspace: "Select project",
  model: "Select model",
  mode: "Select mode",
  thoughtLevel: "Select thinking level",
  reply: "Select reply detail",
} as const;

const telegramCommandNames = {
  bind: "bind",
  help: "help",
  status: "status",
  new: "new",
  workspace: "project",
  model: "model",
  mode: "mode",
  thoughtLevel: "think",
  reply: "reply",
} as const;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

interface TelegramBotIdentity {
  username?: string;
  userId?: string;
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/gu, "\\function splitTelegramText(text: string): string[] {");
}

/**
 * 群聊 @ 检测：文本命中 @botusername，或回复了机器人自己的消息。
 * 身份缺失（getMe 尚未成功）时返回 false——mention 模式会因此静默忽略群消息，
 * 属于安全侧失败；telegram runtime 会在启动时预热身份，正常不会长时间缺失。
 */
function computeTelegramMention(
  message: Record<string, unknown>,
  identity: TelegramBotIdentity | null,
): boolean {
  const text =
    typeof message.text === "string"
      ? message.text
      : typeof message.caption === "string"
        ? message.caption
        : "";
  const username = identity?.username;
  if (username && new RegExp(`@${escapeRegExp(username)}\\b`, "iu").test(text)) {
    return true;
  }
  const replyTo = isRecord(message.reply_to_message) ? message.reply_to_message : null;
  const replyFrom = replyTo && isRecord(replyTo.from) ? replyTo.from : null;
  const replyFromId =
    replyFrom && (typeof replyFrom.id === "number" || typeof replyFrom.id === "string")
      ? String(replyFrom.id)
      : "";
  return Boolean(identity?.userId && replyFromId && replyFromId === identity.userId);
}

function splitTelegramText(text: string): string[] {
  // Telegram 上限 4096 字节；留出余量，并按段落/行边界切分而不是硬切，
  // 避免把代码块、列表或单词拦腰截断（与企微共用同一份分片实现）。
  return splitBotText(text, 3900);
}

function truncateCallbackToast(text: string): string {
  const normalized = text.trim().replace(/\s+/gu, " ");
  return normalized.length > 180 ? `${normalized.slice(0, 177)}...` : normalized;
}

function readTelegramPrivateMessage(
  botId: string,
  update: Record<string, unknown>,
  identity: TelegramBotIdentity | null,
): BotInboundMessage | null {
  const message = isRecord(update.message) ? update.message : null;
  if (!message) {
    return null;
  }
  const chat = isRecord(message.chat) ? message.chat : null;
  const from = isRecord(message.from) ? message.from : null;
  const text =
    typeof message.text === "string"
      ? message.text
      : typeof message.caption === "string"
        ? message.caption
        : "";
  const userId = typeof from?.id === "number" || typeof from?.id === "string" ? String(from.id) : "";
  const chatType = chat?.type === "private" ? "private" : "group";
  const attachments = readTelegramAttachments(message);
  if ((!text && attachments.length === 0) || !userId) {
    return null;
  }
  return {
    botId,
    text,
    ...(attachments.length > 0 ? { attachments } : {}),
    actor: {
      provider: "telegram",
      botId,
      providerUserId: userId,
      displayName:
        typeof from?.username === "string"
          ? from.username
          : typeof from?.first_name === "string"
            ? from.first_name
            : undefined,
      chatType,
      ...(chatType === "group"
        ? { isMention: computeTelegramMention(message, identity) }
        : {}),
      chatId:
        typeof chat?.id === "number" || typeof chat?.id === "string"
          ? String(chat.id)
          : undefined,
      providerMessageId:
        typeof message.message_id === "number" || typeof message.message_id === "string"
          ? String(message.message_id)
          : undefined,
    },
  };
}

function readTelegramFileAttachment(
  value: unknown,
  kind: BotInboundAttachment["kind"],
  fallbackName: string,
): BotInboundAttachment | null {
  if (!isRecord(value)) {
    return null;
  }
  const providerFileId = typeof value.file_id === "string" ? value.file_id : "";
  if (!providerFileId) {
    return null;
  }
  const filename =
    typeof value.file_name === "string" && value.file_name.trim()
      ? value.file_name
      : fallbackName;
  return {
    id: providerFileId,
    kind,
    filename,
    mimeType:
      typeof value.mime_type === "string" && value.mime_type.trim()
        ? value.mime_type
        : defaultMimeTypeForAttachmentKind(kind),
    ...(typeof value.file_size === "number" ? { sizeBytes: value.file_size } : {}),
    providerFileId,
  };
}

function readTelegramPhotoAttachment(message: Record<string, unknown>): BotInboundAttachment | null {
  if (!Array.isArray(message.photo) || message.photo.length === 0) {
    return null;
  }
  const photo = message.photo.filter(isRecord).at(-1);
  if (!photo) {
    return null;
  }
  return readTelegramFileAttachment(photo, "image", "telegram-photo.jpg");
}

function defaultMimeTypeForAttachmentKind(kind: BotInboundAttachment["kind"]): string {
  if (kind === "image") return "image/jpeg";
  if (kind === "audio") return "audio/mpeg";
  if (kind === "video") return "video/mp4";
  return "application/octet-stream";
}

function readTelegramAttachments(message: Record<string, unknown>): BotInboundAttachment[] {
  return [
    readTelegramPhotoAttachment(message),
    readTelegramFileAttachment(message.document, "file", "telegram-document"),
    readTelegramFileAttachment(message.video, "video", "telegram-video.mp4"),
    readTelegramFileAttachment(message.audio, "audio", "telegram-audio"),
    readTelegramFileAttachment(message.voice, "audio", "telegram-voice.ogg"),
  ].filter((attachment): attachment is BotInboundAttachment => attachment !== null);
}

function readTelegramCallbackMessage(botId: string, update: Record<string, unknown>): BotInboundMessage | null {
  const callbackQuery = isRecord(update.callback_query) ? update.callback_query : null;
  if (!callbackQuery) {
    return null;
  }
  const message = isRecord(callbackQuery.message) ? callbackQuery.message : null;
  const chat = isRecord(message?.chat) ? message.chat : null;
  const from = isRecord(callbackQuery.from) ? callbackQuery.from : null;
  const data = typeof callbackQuery.data === "string" ? callbackQuery.data : "";
  const userId = typeof from?.id === "number" || typeof from?.id === "string" ? String(from.id) : "";
  if (!data || !userId) {
    return null;
  }
  const commandText = decodeTelegramCallbackData(data);
  return {
    botId,
    text: commandText,
    actor: {
      provider: "telegram",
      botId,
      providerUserId: userId,
      displayName:
        typeof from?.username === "string"
          ? from.username
          : typeof from?.first_name === "string"
            ? from.first_name
            : undefined,
      chatType: chat?.type === "private" ? "private" : "group",
      chatId:
        typeof chat?.id === "number" || typeof chat?.id === "string"
          ? String(chat.id)
          : undefined,
      providerMessageId:
        typeof callbackQuery.id === "string"
          ? callbackQuery.id
          : typeof message?.message_id === "number" || typeof message?.message_id === "string"
            ? String(message.message_id)
            : undefined,
    },
  };
}

function readTelegramCallbackId(payload: unknown): string | null {
  if (!isRecord(payload)) {
    return null;
  }
  const update = isRecord(payload.update) ? payload.update : payload;
  const callbackQuery = isRecord(update.callback_query) ? update.callback_query : null;
  return typeof callbackQuery?.id === "string" ? callbackQuery.id : null;
}

function readTelegramCallbackMessageRef(payload: unknown): { chatId: string; messageId: number } | null {
  if (!isRecord(payload)) {
    return null;
  }
  const update = isRecord(payload.update) ? payload.update : payload;
  const callbackQuery = isRecord(update.callback_query) ? update.callback_query : null;
  const message = isRecord(callbackQuery?.message) ? callbackQuery.message : null;
  const chat = isRecord(message?.chat) ? message.chat : null;
  const chatId = typeof chat?.id === "number" || typeof chat?.id === "string" ? String(chat.id) : "";
  const messageId = typeof message?.message_id === "number" ? message.message_id : null;
  return chatId && messageId !== null ? { chatId, messageId } : null;
}

function buildSelectionCallbackData(selection: SelectionPrompt, optionId: string, index: number): string {
  if (selection.action === "permission.respond") {
    return `zc:permission:${index + 1}`;
  }
  if (selection.action === "elicitation.respond") {
    return selection.token
      ? `zc:e:${selection.token}:${index + 1}`
      : `zc:elicitation:${index + 1}`;
  }
  if (selection.action === "model.provider.set") {
    return `zc:cmd:/model provider ${index + 1}`;
  }
  if (selection.action === "model.set") {
    return `zc:cmd:/model model ${index + 1}`;
  }
  // Telegram callback_data 最多 64 字节，workspace/task id 可能是远程 identity 或长路径。
  // 这里只回传当前列表序号，后续命令解析复用已有的数字选项解析，避免长 id 被 Telegram 拒收。
  return `zc:${selection.action.replace(".set", "")}:${index + 1}`;
}

function buildSelectionReplyMarkup(selection: SelectionPrompt): { inline_keyboard: Array<Array<{ text: string; callback_data: string }>> } {
  const cancelRows = selection.showCancel === false
    ? []
    : [
        // Bugfix: Telegram 原生按钮以前没有取消入口，用户只能手敲 0 才能退出 pending selection。
        [{ text: selection.cancelLabel ?? "Cancel", callback_data: "zc:cancel" }],
      ];
  return {
    inline_keyboard: [
      ...selection.options.map((option, index) => [
        {
          text: option.label,
          callback_data: buildSelectionCallbackData(selection, option.id, index),
        },
      ]),
      ...cancelRows,
    ],
  };
}

function decodeTelegramCallbackData(data: string): string {
  if (data === "zc:cancel") {
    return "/cancel";
  }
  if (data.startsWith("zc:e:")) {
    const [, , token, value] = data.split(":");
    return token && value ? `/elicitation ${token} ${value}` : "/elicitation";
  }
  if (data.startsWith("zc:cmd:")) {
    return data.slice("zc:cmd:".length);
  }
  if (data.startsWith("zc:")) {
    return `/${data.slice(3).replace(":", " ")}`;
  }
  return data;
}

function buildTelegramCommands(bot: BotConfig): TelegramBotCommand[] {
  return BOT_MENU_COMMAND_ORDER
    .filter((command) => command === "help" || command === "bind" || bot.allowedCommands[command] !== false)
    .map((command) => ({
      command: telegramCommandNames[command],
      description: telegramCommandDescriptions[command],
    }));
}

/**
 * 把流式回复卡片的 blocks 渲染成 Telegram 纯文本。message 块是正文，tools 块
 * （summaries 已在 service 层格式化）渲染为 Markdown 列表；running 状态追加指示符。
 * 导出供单测：节流编辑的正确性完全取决于这份渲染的确定性。不在此截断——
 * 运行中的编辑由调用方截断，终稿超长时走“删除流式消息 + 分段发送全文”。
 */
export function renderStreamingCardStateToText(state: BotStreamingReplyCardState): string {
  const parts: string[] = [];
  for (const block of state.blocks) {
    if (block.type === "message") {
      const text = block.text.trim();
      if (text) {
        parts.push(text);
      }
      continue;
    }
    if (block.summaries.length === 0) {
      continue;
    }
    parts.push(
      [`**${block.title ?? "Tool summaries"}**`, ...block.summaries.map((line) => `- ${line}`)].join(
        "\n",
      ),
    );
  }
  const body = parts.join("\n\n");
  if (state.status === "running" && body) {
    return `${body}\n\n⏳`;
  }
  if (state.status === "error" && body) {
    return `${body}\n\n❌`;
  }
  return body;
}

function truncateTelegramEditText(text: string): string {
  return text.length > TELEGRAM_EDIT_TEXT_LIMIT ? `${text.slice(0, TELEGRAM_EDIT_TEXT_LIMIT)}...` : text;
}

export function createTelegramBotProvider(
  deps: TelegramProviderDeps,
): BotProviderAdapter {
  async function loadToken(bot: BotConfig): Promise<string | null> {
    return bot.credentialRef ? deps.loadCredential(bot.credentialRef) : null;
  }

  // 群聊 @ 检测依赖机器人自身身份（username / user id）；getMe 成功即缓存。
  let cachedBotIdentity: TelegramBotIdentity | null = null;

  async function getMe(bot: BotConfig): Promise<TelegramGetMeResponse | null> {
    const token = await loadToken(bot);
    if (!token?.trim()) {
      return null;
    }
    const response = await fetchBotProviderJson<TelegramGetMeResponse>(
      `https://api.telegram.org/bot${token}/getMe`,
    );
    if (!response.ok) {
      return null;
    }
    const payload = response.payload ?? {};
    const result = payload.result;
    if (result && (result.username || result.id !== undefined)) {
      cachedBotIdentity = {
        ...(result.username ? { username: result.username } : {}),
        ...(result.id !== undefined ? { userId: String(result.id) } : {}),
      };
    }
    return payload;
  }

  /** 上传媒体（sendPhoto/sendDocument 的 multipart）；带超时，失败抛错由服务层呈现。 */
  async function sendMediaMultipart(
    bot: BotConfig,
    method: "sendPhoto" | "sendDocument",
    fields: Record<string, string>,
    fileField: string,
    media: { filename: string; mimeType: string; data: Uint8Array },
  ): Promise<void> {
    const token = await loadToken(bot);
    if (!token?.trim()) {
      throw new Error("Telegram bot token is missing.");
    }
    const form = new FormData();
    for (const [key, value] of Object.entries(fields)) {
      form.append(key, value);
    }
    form.append(
      fileField,
      // 复制为 ArrayBuffer 支撑的视图：Uint8Array<ArrayBufferLike> 与 BlobPart 的泛型不兼容。
      new Blob([Uint8Array.from(media.data)], { type: media.mimeType }),
      media.filename,
    );
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), TELEGRAM_MEDIA_UPLOAD_TIMEOUT_MS);
    try {
      const response = await fetch(`https://api.telegram.org/bot${token}/${method}`, {
        method: "POST",
        body: form,
        signal: controller.signal,
      });
      if (!response.ok) {
        const detail = await response.text().catch(() => "");
        throw new Error(
          `Telegram ${method} failed: HTTP ${response.status}${detail ? ` ${detail.slice(0, 200)}` : ""}`,
        );
      }
    } finally {
      clearTimeout(timeout);
    }
  }

  /** 发送自动分段的 Markdown 文本（解析失败回退纯文本）；send 与流式终稿降级共用。 */
  async function sendSegmentedText(
    bot: BotConfig,
    providerUserId: string,
    text: string,
    replyMarkup?: Record<string, unknown>,
  ): Promise<void> {
    const token = await loadToken(bot);
    if (!token?.trim()) {
      return;
    }
    const chunks = splitTelegramText(text);
    for (const [index, chunk] of chunks.entries()) {
      const shouldAttachReplyMarkup = index === chunks.length - 1 && replyMarkup;
      const response = await fetchBotProvider(`https://api.telegram.org/bot${token}/sendMessage`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          chat_id: providerUserId,
          text: chunk,
          parse_mode: "Markdown",
          ...(shouldAttachReplyMarkup ? { reply_markup: replyMarkup } : {}),
        }),
      });
      if (!response.ok) {
        // Bugfix: Telegram Markdown 对未闭合的 `_*[]()` 很敏感，模型输出偶尔会被拒收。
        // 解析失败时退回纯文本重发，既优先支持 Markdown，也保证消息不会丢。
        await fetchBotProvider(`https://api.telegram.org/bot${token}/sendMessage`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            chat_id: providerUserId,
            text: chunk,
            ...(shouldAttachReplyMarkup ? { reply_markup: replyMarkup } : {}),
          }),
        });
      }
    }
  }

  return {
    async test(bot) {
      if (!bot.credentialRef) {
        return { ok: false, message: "Telegram bot token is missing." };
      }
      const payload = await getMe(bot);
      if (!payload) {
        return { ok: false, message: "Telegram getMe failed." };
      }
      const name = payload.result?.first_name || payload.result?.username;
      return {
        ok: payload.ok === true,
        name,
        message: payload.ok === true ? "Telegram bot is reachable." : payload.description ?? "Telegram getMe failed.",
      };
    },

    async resolveName(bot) {
      const payload = await getMe(bot);
      return payload?.ok === true ? payload.result?.first_name || payload.result?.username || null : null;
    },

    async syncCommands(bot) {
      const token = await loadToken(bot);
      if (!token?.trim()) {
        return;
      }
      if (!bot.enabled) {
        await fetchBotProvider(`https://api.telegram.org/bot${token}/deleteMyCommands`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({}),
        }).catch(() => undefined);
        return;
      }

      const defaultCommands = buildTelegramCommands(bot);
      // Bugfix: Telegram 菜单不会自动从我们支持的 slash commands 推导。
      // 这里只同步默认英文菜单；中文命令由 parser 支持，避免 Telegram 客户端菜单显示中英混杂。
      await fetchBotProvider(`https://api.telegram.org/bot${token}/setMyCommands`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ commands: defaultCommands }),
      });
    },

    async send(bot: BotConfig, message: BotOutboundMessage) {
      const selection = message.selection;
      const replyMarkup = selection ? buildSelectionReplyMarkup(selection) : undefined;
      await sendSegmentedText(bot, message.providerUserId, message.text, replyMarkup);
    },

    async sendMedia(bot, target, media) {
      const fields: Record<string, string> = { chat_id: target.providerUserId };
      if (media.caption?.trim()) {
        fields.caption = media.caption;
      }
      // 图片走 sendPhoto（内联预览），其余走 sendDocument 保留原文件；
      // 大小上限已由服务层按平台能力校验。
      await sendMediaMultipart(
        bot,
        media.kind === "image" ? "sendPhoto" : "sendDocument",
        fields,
        media.kind === "image" ? "photo" : "document",
        media,
      );
    },

    async sendTyping(bot: BotConfig, target) {
      const token = await loadToken(bot);
      if (!token?.trim()) {
        return;
      }
      await fetchBotProvider(`https://api.telegram.org/bot${token}/sendChatAction`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          chat_id: target.providerUserId,
          action: "typing",
        }),
      });
    },

    // 流式回复：Telegram 没有卡片 API，用「sendMessage 拿 message_id + editMessageText 节流编辑」
    // 实现打字机效果。blocks 渲染与超长终稿的分段降级都收口在这里，service 层复用既有节流状态机。
    async createStreamingReplyCard(bot, state, signal) {
      const token = await loadToken(bot);
      if (!token?.trim()) {
        return null;
      }
      const text = truncateTelegramEditText(renderStreamingCardStateToText(state));
      if (!text.trim()) {
        return null;
      }
      const body = {
        chat_id: state.providerUserId,
        text,
        parse_mode: "Markdown",
      };
      const response = await telegramJsonCall<TelegramSendMessageResponse>(
        `https://api.telegram.org/bot${token}/sendMessage`,
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          signal,
          body: JSON.stringify(body),
        },
      );
      let messageId = response.payload?.result?.message_id;
      if (!response.ok) {
        // Bugfix: 同 send() 的 Markdown 回退——模型输出里未闭合的语法会被 Bot API 拒收。
        const plain = await telegramJsonCall<TelegramSendMessageResponse>(
          `https://api.telegram.org/bot${token}/sendMessage`,
          {
            method: "POST",
            headers: { "content-type": "application/json" },
            signal,
            body: JSON.stringify({ chat_id: state.providerUserId, text }),
          },
        );
        messageId = plain.payload?.result?.message_id;
      }
      return messageId !== undefined ? { providerMessageId: String(messageId) } : null;
    },

    async updateStreamingReplyCard(bot, handle, state, signal) {
      const token = await loadToken(bot);
      if (!token?.trim()) {
        return;
      }
      const messageId = Number.parseInt(handle.providerMessageId, 10);
      if (!Number.isFinite(messageId)) {
        return;
      }
      const fullText = renderStreamingCardStateToText(state);
      if (state.status !== "running" && fullText.length > TELEGRAM_EDIT_TEXT_LIMIT) {
        // 终稿超出单条上限：删除流式占位消息，改走 send() 的自动分段全文。
        // 删除失败不阻塞——编辑截断版兜底，内容以随后 send 的全文为准。
        await fetchBotProvider(
          `https://api.telegram.org/bot${token}/deleteMessage`,
          {
            method: "POST",
            headers: { "content-type": "application/json" },
            signal,
            body: JSON.stringify({ chat_id: state.providerUserId, message_id: messageId }),
          },
        ).catch(() => undefined);
        await sendSegmentedText(bot, state.providerUserId, fullText);
        return;
      }
      const text = truncateTelegramEditText(fullText);
      if (!text.trim()) {
        return;
      }
      const response = await telegramJsonCall<TelegramSendMessageResponse>(
        `https://api.telegram.org/bot${token}/editMessageText`,
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          signal,
          body: JSON.stringify({
            chat_id: state.providerUserId,
            message_id: messageId,
            text,
            parse_mode: "Markdown",
          }),
        },
      );
      if (response.ok) {
        return;
      }
      const description = response.payload?.description ?? "";
      // Bugfix: 节流间隔内内容未变化时 Telegram 返回 400 "message is not modified"，
      // 这是幂等成功而非错误；Markdown 拒收才回退纯文本。
      if (description.includes("message is not modified")) {
        return;
      }
      if (description.includes("can't parse entities")) {
        await fetchBotProvider(`https://api.telegram.org/bot${token}/editMessageText`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          signal,
          body: JSON.stringify({
            chat_id: state.providerUserId,
            message_id: messageId,
            text,
          }),
        }).catch(() => undefined);
      }
    },

    async acknowledgeCallback(bot: BotConfig, payload: unknown, text?: string, _message?: BotOutboundMessage, signal?: AbortSignal) {
      const token = await loadToken(bot);
      const callbackQueryId = readTelegramCallbackId(payload);
      if (!token?.trim() || !callbackQueryId) {
        return;
      }
      await fetchBotProvider(`https://api.telegram.org/bot${token}/answerCallbackQuery`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        signal,
        body: JSON.stringify({
          callback_query_id: callbackQueryId,
          ...(text?.trim() ? { text: truncateCallbackToast(text) } : {}),
        }),
      });
      const messageRef = readTelegramCallbackMessageRef(payload);
      if (messageRef && text?.trim()) {
        await fetchBotProvider(`https://api.telegram.org/bot${token}/editMessageReplyMarkup`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          signal,
          body: JSON.stringify({
            chat_id: messageRef.chatId,
            message_id: messageRef.messageId,
            reply_markup: { inline_keyboard: [] },
          }),
        }).catch(() => undefined);
      }
    },

    async downloadAttachment(bot, attachment) {
      const token = await loadToken(bot);
      if (!token?.trim() || !attachment.providerFileId) {
        return null;
      }
      const controller = new AbortController();
      const timeout = setTimeout(
        () => controller.abort(),
        TELEGRAM_ATTACHMENT_DOWNLOAD_TIMEOUT_MS,
      );
      try {
        const fileResponse = await fetch(`https://api.telegram.org/bot${token}/getFile`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ file_id: attachment.providerFileId }),
          signal: controller.signal,
        });
        if (!fileResponse.ok) {
          throw new Error(`Telegram getFile failed: HTTP ${fileResponse.status}`);
        }
        const filePayload = (await fileResponse.json()) as TelegramFileResponse;
        const filePath = filePayload.result?.file_path;
        if (filePayload.ok !== true || !filePath) {
          throw new Error(filePayload.description ?? "Telegram getFile did not return file_path.");
        }
        const response = await fetch(`https://api.telegram.org/file/bot${token}/${filePath}`, {
          signal: controller.signal,
        });
        if (!response.ok) {
          throw new Error(`Telegram file download failed: HTTP ${response.status}`);
        }
        return {
          attachment: {
            ...attachment,
            ...(typeof filePayload.result?.file_size === "number"
              ? { sizeBytes: filePayload.result.file_size }
              : {}),
          },
          data: new Uint8Array(await response.arrayBuffer()),
        };
      } catch (error) {
        if ((error as { name?: unknown })?.name === "AbortError") {
          // Bugfix: Telegram 文件接口卡住时要快速失败，避免 bot 回调一直没有可见结果。
          throw new Error("Telegram file download timed out.");
        }
        throw error;
      } finally {
        clearTimeout(timeout);
      }
    },

    parseCallback(payload): BotInboundMessage[] {
      if (!isRecord(payload)) {
        return [];
      }
      const botId = typeof payload.botId === "string" ? payload.botId : "";
      const update = isRecord(payload.update) ? payload.update : payload;
      if (!botId) {
        return [];
      }
      const message =
        readTelegramPrivateMessage(botId, update, cachedBotIdentity) ??
        readTelegramCallbackMessage(botId, update);
      return message ? [message] : [];
    },
  };
}
