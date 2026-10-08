/* eslint-disable max-lines -- WeCom provider 集中处理智能机器人消息解析、流式回复、附件下载与主动推送。 */
import type {
  BaseMessage,
  FileMessage,
  ImageMessage,
  MixedMessage,
  TextMessage,
  VideoMessage,
  VoiceMessage,
  WsFrame,
} from "@wecom/aibot-node-sdk";
import { generateReqId, WSClient, WSAuthFailureError } from "@wecom/aibot-node-sdk";
import type {
  BotConfig,
  BotInboundAttachment,
  BotInboundMessage,
  BotOutboundMessage,
} from "@mode/shared";
import type { BotProviderAdapter, BotStreamingReplyCardState } from "./types.js";
import { formatBotMessage } from "../messages.js";
import { splitBotText } from "../botText.js";
import { renderStreamingBlocksToMarkdown } from "./streamingText.js";
import type { WeComConnectionRegistry } from "../wecomConnection.js";

/**
 * 企业微信智能机器人 provider（botId + secret，WebSocket 长连接）。
 *
 * 与飞书/Telegram 的差异：
 * - 连接由 wecomChannelRuntime 维护，provider 通过 connection registry 取用；
 * - 主动消息走 aibot_send_msg（markdown），不依赖入站帧；
 * - 流式回复走 aibot_respond_msg + stream（replyStream / replyStreamNonBlocking），
 *   需要透传入站帧的 req_id，因此 create 时按 streamId 记住该会话最近一帧；
 * - 帧不可用（过期/后端推送）时降级为“仅终稿主动推送”，保证内容不丢。
 */

interface WeComProviderDeps {
  loadCredential(key: string): Promise<string | null>;
  connection: WeComConnectionRegistry;
}

/** 单条主动 markdown 消息的字符上限（保守取值，超出自动分片）。 */
export const WECOM_MARKDOWN_CHUNK_LIMIT = 3_500;
/** 流式中间帧的内容上限：超长只展示头部，终稿余量作为后续消息补发。 */
export const WECOM_STREAM_TEXT_LIMIT = 3_500;
/** 认证探测超时；超过按失败处理（用户可重试）。 */
const WECOM_TEST_TIMEOUT_MS = 10_000;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

/** 企微 markdown 分片（边界感知的共享实现见 botText.splitBotText）。 */
export function splitWeComText(text: string, limit = WECOM_MARKDOWN_CHUNK_LIMIT): string[] {
  return splitBotText(text, limit);
}

/**
 * 把流式卡片状态渲染为 WeCom markdown 文本。
 * message 块是正文；tools 块渲染为折叠不能的列表（WeCom 无卡片折叠能力，保持简短）。
 * 导出供单测：流式节流的正确性依赖这份渲染的确定性。
 */
export function renderWeComStreamText(state: BotStreamingReplyCardState): string {
  return renderStreamingBlocksToMarkdown(
    state,
    formatBotMessage(state.locale, "streamingWorking"),
  );
}

function readWeComInboundText(body: BaseMessage): string {
  switch (body.msgtype) {
    case "text":
      return (body as TextMessage).text?.content ?? "";
    case "mixed": {
      const items = (body as MixedMessage).mixed?.msg_item ?? [];
      return items
        .map((item) => (item.msgtype === "text" ? (item.text?.content ?? "") : "[图片]"))
        .filter(Boolean)
        .join("\n");
    }
    case "voice":
      // 语音消息在长连接模式返回转写文本。
      return (body as VoiceMessage).voice?.content ?? "";
    case "image":
      return "[图片]";
    case "file":
      // 文件名不在入站负载里，由 downloadFile 结果补充。
      return "[文件]";
    case "video":
      return "[视频]";
    default:
      return "";
  }
}

function readWeComInboundAttachments(body: BaseMessage): BotInboundAttachment[] {
  const attachments: BotInboundAttachment[] = [];
  const pushMedia = (
    kind: BotInboundAttachment["kind"],
    content: { url?: string; aeskey?: string } | undefined,
    filename: string,
    mimeType: string,
  ): void => {
    if (!content?.url) {
      return;
    }
    attachments.push({
      id: `${kind}-${content.url.slice(-24)}`,
      kind,
      filename,
      mimeType,
      downloadUrl: content.url,
      providerMetadata: content.aeskey ? { aeskey: content.aeskey } : undefined,
    });
  };
  if (body.msgtype === "image") {
    const image = (body as ImageMessage).image;
    pushMedia("image", image, "wecom-image.jpg", "image/jpeg");
  } else if (body.msgtype === "file") {
    const file = (body as FileMessage).file;
    pushMedia("file", file, "wecom-file", "application/octet-stream");
  } else if (body.msgtype === "video") {
    const video = (body as VideoMessage).video;
    pushMedia("video", video, "wecom-video.mp4", "video/mp4");
  } else if (body.msgtype === "mixed") {
    const items = (body as MixedMessage).mixed?.msg_item ?? [];
    items.forEach((item, index) => {
      if (item.msgtype === "image") {
        pushMedia("image", item.image, `wecom-image-${index + 1}.jpg`, "image/jpeg");
      }
    });
  }
  return attachments;
}

export function createWeComBotProvider(deps: WeComProviderDeps): BotProviderAdapter {
  /**
   * streamId → 入站帧（null 表示“仅终稿推送”降级模式）。
   * create 时登记，终稿后清理：流式更新必须复用同一帧的 req_id，
   * 且不受运行时帧缓存的 LRU 淘汰影响。
   */
  const streamFrames = new Map<string, WsFrame | null>();

  async function loadSecret(bot: BotConfig): Promise<string | null> {
    return bot.credentialRef ? deps.loadCredential(bot.credentialRef) : null;
  }

  async function probeAuthentication(botId: string, secret: string): Promise<void> {
    await new Promise<void>((resolve, reject) => {
      const client = new WSClient({
        botId,
        secret,
        maxReconnectAttempts: 0,
        maxAuthFailureAttempts: 1,
        logger: { debug: () => {}, info: () => {}, warn: () => {}, error: () => {} },
      });
      const timeout = setTimeout(() => {
        finish();
        reject(new Error("WeCom authentication probe timed out."));
      }, WECOM_TEST_TIMEOUT_MS);
      let settled = false;
      const finish = (): void => {
        if (settled) {
          return;
        }
        settled = true;
        clearTimeout(timeout);
        client.disconnect();
      };
      client.on("authenticated", () => {
        finish();
        resolve();
      });
      client.on("error", (error: Error) => {
        if (error instanceof WSAuthFailureError) {
          finish();
          reject(new Error("WeCom authentication failed. Check botId and secret."));
        }
      });
      client.connect();
    });
  }

  async function pushMarkdownChunks(
    client: WSClient,
    chatKey: string,
    text: string,
  ): Promise<void> {
    for (const chunk of splitWeComText(text)) {
      await client.sendMessage(chatKey, {
        msgtype: "markdown",
        markdown: { content: chunk },
      });
    }
  }

  return {
    async test(bot) {
      const botId = bot.wecomBotId?.trim() ?? "";
      const secret = await loadSecret(bot);
      if (!botId || !secret?.trim()) {
        return { ok: false, message: "WeCom botId / secret is missing." };
      }
      try {
        await probeAuthentication(botId, secret);
        return { ok: true, name: botId, message: "WeCom bot credentials are valid." };
      } catch (error) {
        return {
          ok: false,
          message: error instanceof Error ? error.message : String(error),
        };
      }
    },

    async send(bot, message: BotOutboundMessage) {
      const connection = deps.connection.getConnection(bot.id);
      if (!connection) {
        throw new Error("WeCom bot is not connected.");
      }
      await pushMarkdownChunks(connection.client, message.providerUserId, message.text);
    },

    // 流式回复：aibot_respond_msg + stream 增量刷新同一条消息。
    async createStreamingReplyCard(bot, state) {
      const connection = deps.connection.getConnection(bot.id);
      const frame = connection?.getLastFrame(state.providerUserId) ?? null;
      const streamId = generateReqId("stream");
      // 无可用帧时进入“仅终稿推送”降级模式：不建流，终稿由 update 走主动推送。
      streamFrames.set(streamId, frame);
      if (!connection || !frame) {
        return { providerMessageId: streamId };
      }
      const text = renderWeComStreamText(state);
      try {
        await connection.client.replyStream(
          frame,
          streamId,
          text.slice(0, WECOM_STREAM_TEXT_LIMIT),
          false,
        );
      } catch {
        // 帧可能已过期：转为推送模式，避免后续继续用失效帧。
        streamFrames.set(streamId, null);
      }
      return { providerMessageId: streamId };
    },

    async updateStreamingReplyCard(bot, handle, state) {
      const connection = deps.connection.getConnection(bot.id);
      const frame = streamFrames.get(handle.providerMessageId) ?? null;
      const streamId = handle.providerMessageId;
      const text = renderWeComStreamText(state);
      const isFinal = state.status !== "running";

      // 推送模式（无帧）：中间态静默，终稿用主动推送保证送达。
      if (!connection || !frame || !frame.headers?.req_id) {
        if (!isFinal) {
          return;
        }
        streamFrames.delete(streamId);
        if (connection) {
          await pushMarkdownChunks(connection.client, state.providerUserId, text);
        }
        return;
      }

      if (!isFinal) {
        // 非阻塞中间帧：上一条同 reqId 未 ack 时 SDK 自动跳过，天然节流。
        await connection.client
          .replyStreamNonBlocking(frame, streamId, text.slice(0, WECOM_STREAM_TEXT_LIMIT), false)
          .catch(() => undefined);
        return;
      }

      streamFrames.delete(streamId);
      const chunks = splitWeComText(text);
      const head = chunks[0] ?? text.slice(0, WECOM_STREAM_TEXT_LIMIT);
      try {
        await connection.client.replyStream(frame, streamId, head, true);
      } catch {
        // 终稿流失败：整段内容改走主动推送，避免内容丢失（如帧过期）。
        await pushMarkdownChunks(connection.client, state.providerUserId, text).catch(
          () => undefined,
        );
        return;
      }
      for (const chunk of chunks.slice(1)) {
        await connection.client
          .sendMessage(state.providerUserId, { msgtype: "markdown", markdown: { content: chunk } })
          .catch(() => undefined);
      }
    },

    parseCallback(payload): BotInboundMessage[] {
      if (!isRecord(payload)) {
        return [];
      }
      const botId = typeof payload.botId === "string" ? payload.botId : "";
      const frame = isRecord(payload.frame) ? (payload.frame as unknown as WsFrame) : null;
      const body = frame?.body as BaseMessage | undefined;
      if (!botId || !body?.msgid || !body.from?.userid) {
        return [];
      }
      const chatType = body.chattype === "group" ? "group" : "private";
      // providerUserId 始终是发送者（授权与身份都基于它）；群聊回复目标由 chatId 表达，
      // createOutbound 会按 chatId 路由到群。
      const text = readWeComInboundText(body);
      const attachments = readWeComInboundAttachments(body);
      if (!text.trim() && attachments.length === 0) {
        return [];
      }
      return [
        {
          botId,
          text,
          ...(attachments.length > 0 ? { attachments } : {}),
          actor: {
            provider: "wecom",
            botId,
            providerUserId: body.from.userid,
            chatType,
            // 企微 AI Bot 平台只在被 @ 时下发群消息回调（回调体没有 @ 字段，也收不到未 @ 的消息），
            // 因此群消息按"定向消息"处理；群聊方式在 UI 里只提供 @提及/关闭。
            ...(chatType === "group" ? { isMention: true } : {}),
            ...(body.chatid ? { chatId: body.chatid } : {}),
            providerMessageId: body.msgid,
          },
        },
      ];
    },

    async sendMedia(bot, target, media) {
      const connection = deps.connection.getConnection(bot.id);
      if (!connection?.client) {
        throw new Error("WeCom bot is not connected.");
      }
      // 先分片上传临时素材（3 天有效），再经 aibot_send_msg 主动推送；
      // 图片内联展示，其余按文件发送并带原文件名。
      const uploaded = await connection.client.uploadMedia(Buffer.from(media.data), {
        type: media.kind === "image" ? "image" : "file",
        filename: media.filename,
      });
      await connection.client.sendMediaMessage(
        target.providerUserId,
        media.kind === "image" ? "image" : "file",
        uploaded.media_id,
        media.kind === "file" ? { title: media.filename } : undefined,
      );
    },

    async downloadAttachment(bot, attachment) {
      const connection = deps.connection.getConnection(bot.id);
      if (!connection?.client || !attachment.downloadUrl) {
        return null;
      }
      const aesKey =
        typeof attachment.providerMetadata?.aeskey === "string"
          ? attachment.providerMetadata.aeskey
          : undefined;
      const { buffer, filename } = await connection.client.downloadFile(
        attachment.downloadUrl,
        aesKey,
      );
      return {
        attachment: {
          ...attachment,
          ...(filename ? { filename } : {}),
        },
        data: new Uint8Array(buffer),
      };
    },
  };
}
