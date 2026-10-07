/* eslint-disable max-lines -- 企微长连接的认证探测、锁竞争、帧缓存、去重与状态上报是一个完整生命周期单元；拆分会让跨进程锁与连接状态的对应关系散落到多个文件。 */
import type {
  BaseMessage,
  Logger as WeComSdkLogger,
  WsFrame,
} from "@wecom/aibot-node-sdk";
import { WSAuthFailureError, WSClient } from "@wecom/aibot-node-sdk";
import type { WeComConnection, WeComConnectionRegistry } from "./wecomConnection.js";
import { resolveWeComChatKey } from "./wecomConnection.js";
import type { BotConfig, BotProviderCallbackResult, BotsConfigFile } from "@zcode/shared";
import type { ICredentialService } from "../credential/credential.js";
import {
  acquireWeComWebSocketLock,
  createLatestRuntimeRefreshQueue,
  type BotRuntimeLogger,
  type BotRuntimeStatusSink,
  waitFor,
} from "./channelRuntime.js";

/**
 * 企业微信智能机器人长连接运行时。
 *
 * 使用腾讯官方 @wecom/aibot-node-sdk（wss://openws.work.weixin.qq.com）：
 * botId + secret 认证后维持一条 WebSocket 长连接，消息经回调帧推送（无需公网回调地址）。
 * 每个 bot 一条连接，并用跨进程文件锁保证同一 botId 只在一个 ZCode 窗口建立连接。
 *
 * 帧缓存：流式回复（aibot_respond_msg）与附件下载都需要透传入站帧的 req_id，
 * 因此连接对象按 chatKey（单聊 userid / 群聊 chatid）保留最近一帧，供 provider 取用。
 */

/** 入站帧按会话保留的最大数量；超出后淘汰最旧，避免长期运行内存增长。 */
const WECOM_LAST_FRAME_LIMIT = 200;
/** 同一 bot 持久化的 msgid 上限与保留时长（覆盖 WS 重连重推窗口）。 */
const WECOM_MESSAGE_DEDUPE_LIMIT = 200;
const WECOM_MESSAGE_DEDUPE_TTL_MS = 24 * 60 * 60 * 1000;
/** 去重表落盘防抖：突发消息合并为一次写盘。 */
const WECOM_DEDUPE_PERSIST_DEBOUNCE_MS = 500;

interface WeComChannelRuntimeDeps {
  runBackgroundTasks?: boolean;
  credentialService: ICredentialService;
  logger: BotRuntimeLogger;
  statusSink: BotRuntimeStatusSink;
  ensureBotStorageMigrated(): Promise<void>;
  readConfig(): Promise<BotsConfigFile>;
  connection: WeComConnectionRegistry;
  /** 跨重启去重：WS 重连时服务端可能重推未回执帧，msgid 需要落盘。 */
  readWecomDedup(botId: string): Promise<Array<{ id: string; at: number }>>;
  writeWecomDedup(botId: string, entries: Array<{ id: string; at: number }>): Promise<void>;
  processProviderCallback(
    provider: "wecom",
    payload: unknown,
  ): Promise<BotProviderCallbackResult>;
}

export function createWeComChannelRuntime(deps: WeComChannelRuntimeDeps) {
  interface RuntimeEntry {
    controller: AbortController;
    fingerprint: string;
    done: Promise<void>;
  }

  const runtimes = new Map<string, RuntimeEntry>();
  const refreshQueue = createLatestRuntimeRefreshQueue();

  function createSdkLogger(botId: string): WeComSdkLogger {
    const prefix = `[wecom] bot=${botId}`;
    return {
      debug: (message: string, ...args: unknown[]) =>
        deps.logger.debug(undefined, `${prefix} ${message} ${formatArgs(args)}`),
      info: (message: string, ...args: unknown[]) =>
        deps.logger.info(undefined, `${prefix} ${message} ${formatArgs(args)}`),
      warn: (message: string, ...args: unknown[]) =>
        deps.logger.warn(undefined, `${prefix} ${message} ${formatArgs(args)}`),
      // SDK 的错误日志走 warn：普通网络抖动会持续重连，不应把运行状态直接推成 error。
      error: (message: string, ...args: unknown[]) =>
        deps.logger.warn(undefined, `${prefix} ${message} ${formatArgs(args)}`),
    };
  }

  function formatArgs(args: readonly unknown[]): string {
    if (args.length === 0) {
      return "";
    }
    return args
      .map((arg) => (typeof arg === "string" ? arg : safeStringify(arg)))
      .join(" ");
  }

  function safeStringify(value: unknown): string {
    try {
      return JSON.stringify(value);
    } catch {
      return String(value);
    }
  }

  async function connectBot(bot: BotConfig, signal: AbortSignal): Promise<void> {
    const secret = bot.credentialRef
      ? await deps.credentialService.load(bot.credentialRef)
      : null;
    const botId = bot.wecomBotId?.trim() ?? "";
    if (!secret?.trim() || !botId) {
      deps.statusSink.setRuntimeStatus({
        botId: bot.id,
        provider: "wecom",
        status: "error",
        message: "WeCom botId or secret is missing.",
      });
      return;
    }

    const lock = await acquireWeComWebSocketLock(bot.id, botId);
    if (!lock) {
      deps.statusSink.setRuntimeStatus({
        botId: bot.id,
        provider: "wecom",
        status: "idle",
        message: "WeCom connection is handled by another ZCode window.",
      });
      // 锁被其他窗口持有：轮询重试，等对方退出后接管。
      while (!signal.aborted) {
        await waitFor(10_000, signal);
        if (signal.aborted) {
          return;
        }
        const retryLock = await acquireWeComWebSocketLock(bot.id, botId);
        if (retryLock) {
          await runConnected(bot, botId, secret, retryLock, signal);
          return;
        }
      }
      return;
    }

    await runConnected(bot, botId, secret, lock, signal);
  }

  async function runConnected(
    bot: BotConfig,
    botId: string,
    secret: string,
    lock: { release(): Promise<void> },
    signal: AbortSignal,
  ): Promise<void> {
    // 去重表从磁盘恢复（跨重启 + 重连窗口内不重复处理）。
    const now0 = Date.now();
    const persisted = await deps.readWecomDedup(bot.id).catch(() => []);
    const seenMessageIds = new Map<string, number>(
      persisted
        .filter((entry) => now0 - entry.at < WECOM_MESSAGE_DEDUPE_TTL_MS)
        .map((entry) => [entry.id, entry.at]),
    );
    let dedupePersistTimer: ReturnType<typeof setTimeout> | null = null;
    const scheduleDedupePersist = (): void => {
      if (dedupePersistTimer) {
        return;
      }
      dedupePersistTimer = setTimeout(() => {
        dedupePersistTimer = null;
        // 只保留最近 N 条，按插入序（Map 迭代序）丢弃最旧。
        const entries = [...seenMessageIds.entries()].map(([id, at]) => ({ id, at }));
        while (entries.length > WECOM_MESSAGE_DEDUPE_LIMIT) {
          entries.shift();
        }
        // 同步裁剪内存表，避免长期运行只涨不降。
        while (seenMessageIds.size > WECOM_MESSAGE_DEDUPE_LIMIT) {
          const oldest = seenMessageIds.keys().next().value;
          if (oldest === undefined) {
            break;
          }
          seenMessageIds.delete(oldest);
        }
        void deps.writeWecomDedup(bot.id, entries).catch((error: unknown) => {
          deps.logger.debug(
            undefined,
            `wecom dedupe persist failed bot=${bot.id}: ${
              error instanceof Error ? error.message : String(error)
            }`,
          );
        });
      }, WECOM_DEDUPE_PERSIST_DEBOUNCE_MS);
      dedupePersistTimer.unref?.();
    };
    const lastFrameByChatKey = new Map<string, WsFrame>();

    const client = new WSClient({
      botId,
      secret,
      // 桌面端长期运行：连接抖动交给 SDK 无限指数退避重连；认证失败由 authFailure 处理。
      maxReconnectAttempts: -1,
      maxAuthFailureAttempts: 5,
      logger: createSdkLogger(bot.id),
    });

    const connection: WeComConnection = {
      client,
      getLastFrame(chatKey: string) {
        return lastFrameByChatKey.get(chatKey) ?? null;
      },
    };
    deps.connection.setConnection(bot.id, connection);

    const rememberFrame = (message: BaseMessage, frame: WsFrame): void => {
      const chatKey = resolveWeComChatKey(message);
      // 重新 set 让该键回到 Map 迭代序末尾（Map 保持插入序，可当 LRU 用）。
      lastFrameByChatKey.delete(chatKey);
      lastFrameByChatKey.set(chatKey, frame);
      while (lastFrameByChatKey.size > WECOM_LAST_FRAME_LIMIT) {
        const oldestKey = lastFrameByChatKey.keys().next().value;
        if (oldestKey === undefined) {
          break;
        }
        lastFrameByChatKey.delete(oldestKey);
      }
    };

    const isDuplicate = (messageId: string | undefined): boolean => {
      if (!messageId) {
        return false;
      }
      if (seenMessageIds.has(messageId)) {
        return true;
      }
      seenMessageIds.set(messageId, Date.now());
      scheduleDedupePersist();
      return false;
    };

    const handleInbound = (frame: WsFrame): void => {
      const message = frame.body as BaseMessage | undefined;
      if (!message?.msgid || !message.from?.userid) {
        return;
      }
      rememberFrame(message, frame);
      if (isDuplicate(message.msgid)) {
        deps.logger.debug(
          undefined,
          `wecom duplicate message ignored bot=${bot.id} msgid=${message.msgid}`,
        );
        return;
      }
      deps.statusSink.setRuntimeStatus({
        botId: bot.id,
        provider: "wecom",
        status: "connected",
        messageId: "bots.runtime.wecomConnected",
        message: "WeCom bot is connected.",
      });
      void deps
        .processProviderCallback("wecom", { botId: bot.id, frame })
        .then((result) => {
          if (!result.ok) {
            deps.logger.warn(
              undefined,
              `wecom callback not ok bot=${bot.id} status=${result.status ?? "unknown"}`,
            );
          }
        })
        .catch((error: unknown) => {
          deps.logger.warn(
            undefined,
            `wecom callback failed bot=${bot.id}: ${
              error instanceof Error ? error.message : String(error)
            }`,
          );
        });
    };

    let authFailed = false;
    let kicked = false;

    client.on("connected", () => {
      deps.statusSink.setRuntimeStatus({
        botId: bot.id,
        provider: "wecom",
        status: "polling",
        messageId: "bots.runtime.wecomConnecting",
        message: "WeCom bot is connecting.",
      });
    });
    client.on("authenticated", () => {
      deps.statusSink.setRuntimeStatus({
        botId: bot.id,
        provider: "wecom",
        status: "connected",
        messageId: "bots.runtime.wecomConnected",
        message: "WeCom bot is connected.",
      });
    });
    client.on("disconnected", (reason) => {
      deps.logger.debug(undefined, `wecom disconnected bot=${bot.id} reason=${reason}`);
    });
    client.on("reconnecting", (attempt) => {
      deps.statusSink.setRuntimeStatus({
        botId: bot.id,
        provider: "wecom",
        status: "polling",
        messageId: "bots.runtime.wecomReconnecting",
        message: `WeCom bot is reconnecting (attempt ${attempt}).`,
      });
    });
    client.on("error", (error) => {
      if (error instanceof WSAuthFailureError) {
        // botId/secret 错误：重连无法恢复，明确报错让用户重新绑定。
        authFailed = true;
        deps.statusSink.setRuntimeStatus({
          botId: bot.id,
          provider: "wecom",
          status: "error",
          message: "WeCom authentication failed. Check botId and secret.",
        });
        return;
      }
      deps.logger.warn(
        undefined,
        `wecom connection error bot=${bot.id}: ${error.message}`,
      );
    });
    client.on("event.disconnected_event", () => {
      // 同一 botId 在别处建立了新连接（例如另一个 ZCode 窗口）；停止本窗口连接避免互踢。
      kicked = true;
      deps.statusSink.setRuntimeStatus({
        botId: bot.id,
        provider: "wecom",
        status: "error",
        messageId: "bots.runtime.wecomKicked",
        message: "WeCom connection was taken over by another client.",
      });
    });
    client.on("message.text", handleInbound);
    client.on("message.image", handleInbound);
    client.on("message.mixed", handleInbound);
    client.on("message.voice", handleInbound);
    client.on("message.file", handleInbound);
    client.on("message.video", handleInbound);

    try {
      client.connect();
      // 连接建立后由 SDK 自行维持；这里只等待停止信号或不可恢复的终态。
      while (!signal.aborted && !authFailed && !kicked) {
        await waitFor(1_000, signal);
      }
    } finally {
      if (dedupePersistTimer) {
        clearTimeout(dedupePersistTimer);
        dedupePersistTimer = null;
      }
      // 断开前冲刷一次，保证"已处理"记录不因退出丢失。
      const flushEntries = [...seenMessageIds.entries()]
        .slice(-WECOM_MESSAGE_DEDUPE_LIMIT)
        .map(([id, at]) => ({ id, at }));
      await deps.writeWecomDedup(bot.id, flushEntries).catch(() => undefined);
      deps.connection.clearConnection(bot.id, connection);
      client.disconnect();
      await lock.release().catch((error: unknown) => {
        deps.logger.debug(
          undefined,
          `release wecom lock failed bot=${bot.id}: ${
            error instanceof Error ? error.message : String(error)
          }`,
        );
      });
      const previous = deps.statusSink.getRuntimeStatus(bot.id);
      if (previous && !authFailed && !kicked) {
        deps.statusSink.setRuntimeStatus({
          ...previous,
          status: "idle",
          messageId: "bots.runtime.wecomStopped",
          message: "WeCom bot connection is stopped.",
        });
      }
    }
  }

  async function getConnectionFingerprint(bot: BotConfig): Promise<string> {
    const credential = bot.credentialRef
      ? await deps.credentialService.load(bot.credentialRef)
      : null;
    return [bot.provider, bot.id, bot.wecomBotId?.trim() ?? "", credential ?? ""].join("::");
  }

  async function stopConnection(botId: string): Promise<void> {
    const runtime = runtimes.get(botId);
    runtime?.controller.abort();
    if (runtime) {
      await runtime.done;
      if (runtimes.get(botId) === runtime) {
        runtimes.delete(botId);
      }
    }
  }

  function startConnection(bot: BotConfig, fingerprint: string): void {
    if (runtimes.has(bot.id)) {
      return;
    }
    const controller = new AbortController();
    deps.statusSink.setRuntimeStatus({
      botId: bot.id,
      provider: "wecom",
      status: "polling",
      messageId: "bots.runtime.wecomConnecting",
      message: "WeCom bot is connecting.",
    });
    const runtime: RuntimeEntry = {
      controller,
      fingerprint,
      done: Promise.resolve(),
    };
    runtime.done = connectBot(bot, controller.signal).finally(() => {
      if (runtimes.get(bot.id) === runtime) {
        runtimes.delete(bot.id);
      }
    });
    runtimes.set(bot.id, runtime);
  }

  async function reconcile(
    config: BotsConfigFile | undefined,
    isLatest: () => boolean,
  ): Promise<void> {
    await deps.ensureBotStorageMigrated();
    const currentConfig = config ?? (await deps.readConfig());
    if (!isLatest()) {
      return;
    }
    const activeIds = new Set(
      currentConfig.bots
        .filter((bot) => bot.provider === "wecom" && bot.enabled && bot.credentialRef)
        .map((bot) => bot.id),
    );
    for (const botId of runtimes.keys()) {
      if (!activeIds.has(botId)) {
        await stopConnection(botId);
        if (!isLatest()) {
          return;
        }
      }
    }
    for (const bot of currentConfig.bots) {
      if (bot.provider === "wecom" && bot.enabled && bot.credentialRef) {
        const fingerprint = await getConnectionFingerprint(bot);
        if (!isLatest()) {
          return;
        }
        const runtime = runtimes.get(bot.id);
        if (runtime && runtime.fingerprint !== fingerprint) {
          // 凭据或 botId 变化：旧连接闭包持有旧 secret，必须先停再起。
          await stopConnection(bot.id);
          if (!isLatest()) {
            return;
          }
        }
        startConnection(bot, fingerprint);
      } else if (bot.provider === "wecom" && !bot.enabled) {
        deps.statusSink.setRuntimeStatus({
          botId: bot.id,
          provider: "wecom",
          status: "disabled",
          messageId: "bots.runtime.botDisabled",
          message: "Bot is disabled.",
        });
      }
    }
  }

  function refresh(config?: BotsConfigFile): Promise<void> {
    return refreshQueue.enqueue((isLatest) => reconcile(config, isLatest));
  }

  function scheduleRefresh(config?: BotsConfigFile): void {
    if (deps.runBackgroundTasks === false) {
      // 与其它 channel runtime 一致：attached remote 只暴露控制面，不起本地长连接。
      return;
    }
    void refresh(config).catch((error: unknown) => {
      deps.logger.warn(
        undefined,
        `refresh WeCom connection failed: ${
          error instanceof Error ? error.message : String(error)
        }`,
      );
    });
  }

  async function dispose(): Promise<void> {
    refreshQueue.invalidate();
    const activeRuntimes = [...runtimes.values()];
    for (const runtime of activeRuntimes) {
      runtime.controller.abort();
    }
    await Promise.allSettled(activeRuntimes.map((runtime) => runtime.done));
    for (const [botId, runtime] of runtimes) {
      if (activeRuntimes.includes(runtime)) {
        runtimes.delete(botId);
      }
    }
  }

  return {
    dispose,
    refresh,
    scheduleRefresh,
    stopConnection,
  };
}
