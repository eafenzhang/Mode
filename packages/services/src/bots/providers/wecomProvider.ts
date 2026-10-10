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
import type {
  BotProviderAdapter,
  BotStreamingReplyCardState,
  BotTypingTarget,
} from "./types.js";
import { formatBotMessage } from "../messages.js";
import { splitBotText } from "../botText.js";
import { renderStreamingBlocksToMarkdown } from "./streamingText.js";
import type { WeComConnection, WeComConnectionRegistry } from "../wecomConnection.js";

/**
 * 企业微信智能机器人 provider（botId + secret，WebSocket 长连接）。
 *
 * 与飞书/Telegram 的差异：
 * - 连接由 wecomChannelRuntime 维护，provider 通过 connection registry 取用；
 * - 主动消息走 aibot_send_msg（markdown），不依赖入站帧；
 * - 流式回复走 aibot_respond_msg + stream（replyStream / replyStreamNonBlocking），
 *   需要透传入站帧的 req_id，因此 create 时按 streamId 记住该会话最近一帧；
 * - 帧不可用（过期/后端推送）时降级为“仅终稿主动推送”，保证内容不丢。
 *
 * 等待期反馈（typing）：
 * - 企微 SDK 没有原生「正在输入」命令（对照飞书 Typing reaction / Telegram chatAction），
 *   等待期的可见反馈只能靠立刻开一条非终态 replyStream 占位（官方 SDK 示例同款做法）；
 * - 占位流以同流刷新模拟桌面三点气泡的 · → ·· → ··· 打点动画（非终态帧原地更新同一条消息）；
 * - 占位流没有撤回 API，stopTyping 只停动画、不收口——收口归属给下一条真正出站的消息：
 *   send 接管成正文首片，流式卡片接管成卡片首帧；无出站时由下一轮出站接管。
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
/**
 * 等待期打点动画的帧与节奏：与桌面三点气泡同观感（点逐个出现后重开一轮）。
 * 企微只能靠同流刷新模拟动画——平台没有动画素材通道，帧间 500ms 是
 * 「观感流畅」与「WS 帧开销」的折中；上一帧未 ack 时 NonBlocking 自动跳过，天然节流。
 */
const TYPING_ANIMATION_FRAMES = ["·", "··", "···"] as const;
const TYPING_ANIMATION_FRAME_MS = 500;

interface PendingTypingStream {
  frame: WsFrame;
  streamId: string;
  /** 停打点动画（收口、stopTyping、任务冻结时调用）；不负责关流。 */
  stopAnimation?: () => void;
}

/** 启动打点动画：同一条占位流按 · → ·· → ··· 循环刷新非终态帧。 */
function startTypingAnimation(
  pending: PendingTypingStream,
  client: WeComConnection["client"],
): void {
  if (pending.stopAnimation) {
    return;
  }
  let index = 0;
  const timer = setInterval(() => {
    index = (index + 1) % TYPING_ANIMATION_FRAMES.length;
    void client
      .replyStreamNonBlocking(
        pending.frame,
        pending.streamId,
        TYPING_ANIMATION_FRAMES[index]!,
        false,
      )
      .catch(() => undefined);
  }, TYPING_ANIMATION_FRAME_MS);
  timer.unref?.();
  pending.stopAnimation = () => clearInterval(timer);
}

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

  /**
   * `${botId}:${chatKey}` → 已打开但尚未被真正回复接管的占位流。
   * provider 实例按 provider 名共享（同一实例服务全部企微 bot），键必须带 botId，
   * 否则同一会话里多个 bot 会互相收口对方的占位流。
   */
  const pendingTypingStreams = new Map<string, PendingTypingStream>();
  // 打开占位是异步的：并发的 startTyping/sendTyping 用 in-flight promise 防开出第二条流；
  // 出站/建卡必须等它收敛再认领，否则会出现「回复先落地、占位帧后到」的残留。
  const typingOpenInFlight = new Map<string, Promise<void>>();

  function typingStreamKey(bot: BotConfig, chatKey: string): string {
    return `${bot.id}:${chatKey}`;
  }

  /** 等待期反馈：立刻开非终态占位流并起打点动画（企微没有原生 typing 信号，SDK 也没有撤回 API）。 */
  async function ensureTypingPlaceholder(bot: BotConfig, chatKey: string): Promise<void> {
    const key = typingStreamKey(bot, chatKey);
    const existing = pendingTypingStreams.get(key);
    if (existing) {
      // 任务恢复（问答/权限通过后再次 startTyping）时把停掉的动画续上。
      const connection = deps.connection.getConnection(bot.id);
      if (connection) {
        startTypingAnimation(existing, connection.client);
      }
      return;
    }
    const inFlight = typingOpenInFlight.get(key);
    if (inFlight) {
      await inFlight;
      return;
    }
    const connection = deps.connection.getConnection(bot.id);
    const frame = connection?.getLastFrame(chatKey) ?? null;
    if (!connection || !frame) {
      // 主动触达/后端推送没有入站帧：拿不到 req_id 就开不了流，保持静默降级。
      return;
    }
    const open = (async () => {
      try {
        const streamId = generateReqId("stream");
        // 首帧即打点第一帧「·」：等待期立刻有可见反馈，动画随后同流刷新。
        await connection.client.replyStream(frame, streamId, TYPING_ANIMATION_FRAMES[0], false);
        const pending: PendingTypingStream = { frame, streamId };
        startTypingAnimation(pending, connection.client);
        pendingTypingStreams.set(key, pending);
      } catch {
        // 占位是 best-effort：帧过期或平台拒绝时静默放弃，不影响真实回复链路。
      } finally {
        typingOpenInFlight.delete(key);
      }
    })();
    typingOpenInFlight.set(key, open);
    await open;
  }

  /** 出站/建卡在认领前先等进行中的打开收敛，避免认领落空留下迟到的占位帧。 */
  async function waitPendingTypingOpen(bot: BotConfig, chatKey: string): Promise<void> {
    const inFlight = typingOpenInFlight.get(typingStreamKey(bot, chatKey));
    if (inFlight) {
      await inFlight;
    }
  }

  /** 出站/卡片建流前认领占位流；认领即停动画，收口由调用方负责，避免重复消费。 */
  function takePendingTypingStream(bot: BotConfig, chatKey: string): PendingTypingStream | undefined {
    const key = typingStreamKey(bot, chatKey);
    const pending = pendingTypingStreams.get(key);
    if (pending) {
      pendingTypingStreams.delete(key);
      pending.stopAnimation?.();
    }
    return pending;
  }

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
      const chunks = splitWeComText(message.text);
      if (chunks.length > 0) {
        await waitPendingTypingOpen(bot, message.providerUserId);
        const pending = takePendingTypingStream(bot, message.providerUserId);
        if (pending) {
          try {
            // 接管占位流：同一条消息被正文首片替换并收口，用户侧看不到第二条消息。
            await connection.client.replyStream(pending.frame, pending.streamId, chunks[0]!, true);
            for (const chunk of chunks.slice(1)) {
              await connection.client.sendMessage(message.providerUserId, {
                msgtype: "markdown",
                markdown: { content: chunk },
              });
            }
            return;
          } catch {
            // 收口失败通常意味着帧已过期、这条流已不可寻址：归还只会让占位流
            // 永久卡死后续 typing 打开。放弃它、回落主动推送；内容不丢，
            // 下一次 typing 会用新入站帧开新占位流。
          }
        }
      }
      await pushMarkdownChunks(connection.client, message.providerUserId, message.text);
    },

    // 等待期反馈：开非终态占位流（企微无原生 typing 信号）。长任务由 startTyping 触发
    // 一次；同步短命令走 sendTyping。两者共用同一幂等打开逻辑，重复触发不会开第二条流。
    async sendTyping(bot: BotConfig, target: BotTypingTarget) {
      await ensureTypingPlaceholder(bot, target.providerUserId);
    },

    async startTyping(bot: BotConfig, target: BotTypingTarget) {
      await ensureTypingPlaceholder(bot, target.providerUserId);
    },

    // 只停打点动画、不收口：占位流没有撤回 API，这里 finish 会把打点文案留成永久消息。
    // 冻结的点由紧随其后的真正出站接管（send/流式卡片），任务恢复时 startTyping 会续上动画。
    async stopTyping(bot: BotConfig, target: BotTypingTarget) {
      pendingTypingStreams.get(typingStreamKey(bot, target.providerUserId))?.stopAnimation?.();
    },

    // 流式回复：aibot_respond_msg + stream 增量刷新同一条消息。
    async createStreamingReplyCard(bot, state) {
      const connection = deps.connection.getConnection(bot.id);
      const text = renderWeComStreamText(state);
      // 等待期占位流已就位：复用同一 streamId，卡片首帧替换占位文案。
      // 同一条消息原地更新，用户侧不会出现「正在处理...」与卡片两条消息。
      let pending: { frame: WsFrame; streamId: string } | undefined;
      if (connection) {
        await waitPendingTypingOpen(bot, state.providerUserId);
        pending = takePendingTypingStream(bot, state.providerUserId);
      }
      if (pending) {
        try {
          await connection!.client.replyStream(
            pending.frame,
            pending.streamId,
            text.slice(0, WECOM_STREAM_TEXT_LIMIT),
            false,
          );
          streamFrames.set(pending.streamId, pending.frame);
          return { providerMessageId: pending.streamId };
        } catch {
          // 占位首帧失败（帧过期）：不复用失效流，落入下面的常规创建/降级路径。
          // 占位流不归还——同帧重建同样会失败，归还只会卡死后续 typing 打开。
        }
      }
      const frame = connection?.getLastFrame(state.providerUserId) ?? null;
      const streamId = generateReqId("stream");
      // 无可用帧时进入“仅终稿推送”降级模式：不建流，终稿由 update 走主动推送。
      streamFrames.set(streamId, frame);
      if (!connection || !frame) {
        return { providerMessageId: streamId };
      }
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
