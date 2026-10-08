/* eslint-disable max-lines -- 钉钉 Stream 长连接的注册、帧协议、去重与重连是一个完整生命周期单元。 */
import WebSocket from "ws";
import type { BotConfig, BotProviderCallbackResult, BotsConfigFile } from "@mode/shared";
import type { ICredentialService } from "../credential/credential.js";
import { DINGTALK_API_BASE } from "./providers/dingtalkProvider.js";
import { fetchBotProviderJson } from "./providers/providerRequest.js";
import {
  createLatestRuntimeRefreshQueue,
  type BotRuntimeLogger,
  type BotRuntimeStatusSink,
  waitFor,
} from "./channelRuntime.js";

/**
 * 钉钉 Stream 模式长连接运行时（对齐 MyAgents 的 dingtalk 连接管理）。
 *
 * 1) POST /v1.0/gateway/connections/open 注册订阅 → { endpoint, ticket }
 * 2) WS 连接 `{endpoint}?ticket={ticket}`
 * 3) 帧协议（JSON text）：
 *    - SYSTEM/ping → 原样回 pong（headers + data 透传，code 200）
 *    - SYSTEM/disconnect → 服务端要求断开，交给重连循环
 *    - CALLBACK/EVENT → 回 ACK（EVENT 的 data 需带 {"status":"SUCCESS"}）
 *    - CALLBACK topic /v1.0/im/bot/messages/get（含尾斜杠变体）→ 交给 provider 解析
 * 4) 保活：120 秒无任何帧判定死连接，指数退避重连（1s → 60s 上限）
 *
 * 去重：msgId 落盘（跨重启），覆盖服务端重投窗口。
 */

const WS_READ_TIMEOUT_MS = 120_000;
const RECONNECT_BACKOFF_MIN_MS = 1_000;
const RECONNECT_BACKOFF_MAX_MS = 60_000;
/** 连接存活超过该时长后重置退避（避免惩罚健康连接）。 */
const HEALTHY_CONNECTION_MS = 30_000;
const MESSAGE_DEDUPE_LIMIT = 200;
const MESSAGE_DEDUPE_TTL_MS = 24 * 60 * 60 * 1000;
const DEDUPE_PERSIST_DEBOUNCE_MS = 500;

interface DingtalkChannelRuntimeDeps {
  runBackgroundTasks?: boolean;
  credentialService: ICredentialService;
  logger: BotRuntimeLogger;
  statusSink: BotRuntimeStatusSink;
  ensureBotStorageMigrated(): Promise<void>;
  readConfig(): Promise<BotsConfigFile>;
  readDingtalkDedup(botId: string): Promise<Array<{ id: string; at: number }>>;
  writeDingtalkDedup(botId: string, entries: Array<{ id: string; at: number }>): Promise<void>;
  processProviderCallback(
    provider: "dingtalk",
    payload: unknown,
  ): Promise<BotProviderCallbackResult>;
}

interface DingtalkStreamRegistration {
  endpoint?: string;
  ticket?: string;
}

export function createDingtalkChannelRuntime(deps: DingtalkChannelRuntimeDeps) {
  interface RuntimeEntry {
    controller: AbortController;
    fingerprint: string;
    done: Promise<void>;
  }

  const runtimes = new Map<string, RuntimeEntry>();
  const refreshQueue = createLatestRuntimeRefreshQueue();

  async function getConnectionFingerprint(bot: BotConfig): Promise<string> {
    const credential = bot.credentialRef
      ? await deps.credentialService.load(bot.credentialRef)
      : null;
    return [bot.provider, bot.id, bot.dingtalkClientId?.trim() ?? "", credential ?? ""].join("::");
  }

  async function registerStreamConnection(
    bot: BotConfig,
  ): Promise<DingtalkStreamRegistration | null> {
    const clientId = bot.dingtalkClientId?.trim();
    const clientSecret = bot.credentialRef
      ? await deps.credentialService.load(bot.credentialRef)
      : null;
    if (!clientId || !clientSecret?.trim()) {
      return null;
    }
    const response = await fetchBotProviderJson<DingtalkStreamRegistration>(
      `${DINGTALK_API_BASE}/v1.0/gateway/connections/open`,
      {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          clientId,
          clientSecret: clientSecret.trim(),
          subscriptions: [
            { type: "CALLBACK", topic: "/v1.0/im/bot/messages/get" },
            { type: "EVENT", topic: "*" },
          ],
        }),
      },
    );
    if (!response.ok || !response.payload?.endpoint || !response.payload?.ticket) {
      return null;
    }
    return response.payload;
  }

  async function runConnection(bot: BotConfig, signal: AbortSignal): Promise<void> {
    // 去重表：从磁盘恢复（跨重启 + 服务端重投窗口）。
    const now0 = Date.now();
    const persisted = await deps.readDingtalkDedup(bot.id).catch(() => []);
    const seenMessageIds = new Map<string, number>(
      persisted
        .filter((entry) => now0 - entry.at < MESSAGE_DEDUPE_TTL_MS)
        .map((entry) => [entry.id, entry.at]),
    );
    let dedupeTimer: ReturnType<typeof setTimeout> | null = null;
    const scheduleDedupePersist = (): void => {
      if (dedupeTimer) {
        return;
      }
      dedupeTimer = setTimeout(() => {
        dedupeTimer = null;
        const entries = [...seenMessageIds.entries()].slice(-MESSAGE_DEDUPE_LIMIT).map(([id, at]) => ({ id, at }));
        while (seenMessageIds.size > MESSAGE_DEDUPE_LIMIT) {
          const oldest = seenMessageIds.keys().next().value;
          if (oldest === undefined) break;
          seenMessageIds.delete(oldest);
        }
        void deps.writeDingtalkDedup(bot.id, entries).catch((error: unknown) => {
          deps.logger.debug(
            undefined,
            `dingtalk dedupe persist failed bot=${bot.id}: ${
              error instanceof Error ? error.message : String(error)
            }`,
          );
        });
      }, DEDUPE_PERSIST_DEBOUNCE_MS);
      dedupeTimer.unref?.();
    };
    const isDuplicate = (messageId: string): boolean => {
      if (seenMessageIds.has(messageId)) {
        return true;
      }
      seenMessageIds.set(messageId, Date.now());
      scheduleDedupePersist();
      return false;
    };

    const handleTextFrame = (socket: WebSocket, raw: string): void => {
      let frame: Record<string, unknown>;
      try {
        const parsed: unknown = JSON.parse(raw);
        if (typeof parsed !== "object" || parsed === null) {
          return;
        }
        frame = parsed as Record<string, unknown>;
      } catch {
        return;
      }
      const type = typeof frame.type === "string" ? frame.type : "";
      const headers = typeof frame.headers === "object" && frame.headers !== null
        ? (frame.headers as Record<string, unknown>)
        : {};
      const messageId = typeof headers.messageId === "string" ? headers.messageId : "";
      const topic = typeof headers.topic === "string" ? headers.topic : "";

      if (type === "SYSTEM") {
        if (topic === "ping") {
          // 原样回 pong（headers + data 透传），对齐 MyAgents。
          socket.send(
            JSON.stringify({ code: 200, headers: frame.headers, message: "OK", data: frame.data }),
          );
          return;
        }
        if (topic === "disconnect") {
          deps.logger.info(
            undefined,
            `dingtalk server disconnect notice bot=${bot.id} reason=${
              typeof frame.data === "string" ? frame.data : "unknown"
            }`,
          );
          socket.close();
          return;
        }
        return;
      }

      if ((type === "CALLBACK" || type === "EVENT") && messageId) {
        // EVENT 的 ACK 需要携带 status；CALLBACK 空对象即可（对齐 MyAgents）。
        const ackData =
          type === "EVENT" ? JSON.stringify({ status: "SUCCESS", message: "success" }) : "{}";
        socket.send(
          JSON.stringify({
            code: 200,
            headers: { contentType: "application/json", messageId },
            message: "OK",
            data: ackData,
          }),
        );
      }

      if (type !== "CALLBACK") {
        return;
      }
      if (topic !== "/v1.0/im/bot/messages/get" && topic !== "/v1.0/im/bot/messages/get/") {
        return;
      }
      // 去重：先解析 msgId，重复帧直接跳过（ACK 已发，不会触发服务端重投）。
      let msgId = "";
      try {
        const data = typeof frame.data === "string" ? JSON.parse(frame.data) : null;
        if (typeof data === "object" && data !== null && typeof (data as { msgId?: unknown }).msgId === "string") {
          msgId = (data as { msgId: string }).msgId;
        }
      } catch {
        return;
      }
      if (!msgId || isDuplicate(msgId)) {
        return;
      }
      void deps
        .processProviderCallback("dingtalk", { botId: bot.id, frame })
        .catch((error: unknown) => {
          deps.logger.warn(
            undefined,
            `dingtalk callback failed bot=${bot.id}: ${
              error instanceof Error ? error.message : String(error)
            }`,
          );
        });
    };

    let backoffMs = RECONNECT_BACKOFF_MIN_MS;
    while (!signal.aborted) {
      const registration = await registerStreamConnection(bot).catch(() => null);
      if (!registration?.endpoint || !registration.ticket) {
        deps.statusSink.setRuntimeStatus({
          botId: bot.id,
          provider: "dingtalk",
          status: "error",
          message: "DingTalk stream registration failed.",
        });
        await waitFor(5_000, signal);
        continue;
      }

      const connectionStart = Date.now();
      await new Promise<void>((resolve) => {
        const socket = new WebSocket(`${registration.endpoint}?ticket=${registration.ticket}`);
        let lastActivity = Date.now();
        let settled = false;
        const finish = (): void => {
          if (settled) return;
          settled = true;
          clearInterval(readTimeoutTimer);
          signal.removeEventListener("abort", onAbort);
          try {
            socket.close();
          } catch {
            // 关闭失败忽略：连接已不可用。
          }
          resolve();
        };
        const onAbort = (): void => finish();
        const readTimeoutTimer = setInterval(() => {
          if (Date.now() - lastActivity > WS_READ_TIMEOUT_MS) {
            deps.logger.warn(
              undefined,
              `dingtalk WS read timeout (no frame for ${WS_READ_TIMEOUT_MS / 1000}s) bot=${bot.id}`,
            );
            finish();
          }
        }, 5_000);
        readTimeoutTimer.unref?.();
        signal.addEventListener("abort", onAbort, { once: true });

        socket.on("open", () => {
          deps.statusSink.setRuntimeStatus({
            botId: bot.id,
            provider: "dingtalk",
            status: "polling",
            messageId: "bots.runtime.dingtalkConnected",
            message: "DingTalk stream is connected.",
          });
        });
        socket.on("message", (raw) => {
          lastActivity = Date.now();
          handleTextFrame(socket, typeof raw === "string" ? raw : raw.toString("utf8"));
        });
        socket.on("error", (error: Error) => {
          deps.logger.warn(undefined, `dingtalk WS error bot=${bot.id}: ${error.message}`);
        });
        socket.on("close", () => {
          finish();
        });
      });

      if (signal.aborted) {
        return;
      }
      // 健康连接（活过 30 秒）重置退避；否则指数退避到 60 秒上限。
      backoffMs =
        Date.now() - connectionStart > HEALTHY_CONNECTION_MS
          ? RECONNECT_BACKOFF_MIN_MS
          : Math.min(backoffMs * 2, RECONNECT_BACKOFF_MAX_MS);
      deps.statusSink.setRuntimeStatus({
        botId: bot.id,
        provider: "dingtalk",
        status: "polling",
        messageId: "bots.runtime.dingtalkReconnecting",
        message: "DingTalk stream is reconnecting.",
      });
      await waitFor(backoffMs, signal);
    }
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
      provider: "dingtalk",
      status: "polling",
      messageId: "bots.runtime.dingtalkConnecting",
      message: "DingTalk stream is connecting.",
    });
    const runtime: RuntimeEntry = {
      controller,
      fingerprint,
      done: Promise.resolve(),
    };
    runtime.done = runConnection(bot, controller.signal).finally(() => {
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
        .filter(
          (bot) => bot.provider === "dingtalk" && bot.enabled && bot.credentialRef,
        )
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
      if (bot.provider === "dingtalk" && bot.enabled && bot.credentialRef) {
        const fingerprint = await getConnectionFingerprint(bot);
        if (!isLatest()) {
          return;
        }
        const runtime = runtimes.get(bot.id);
        if (runtime && runtime.fingerprint !== fingerprint) {
          await stopConnection(bot.id);
          if (!isLatest()) {
            return;
          }
        }
        startConnection(bot, fingerprint);
      } else if (bot.provider === "dingtalk" && !bot.enabled) {
        deps.statusSink.setRuntimeStatus({
          botId: bot.id,
          provider: "dingtalk",
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
      return;
    }
    void refresh(config).catch((error: unknown) => {
      deps.logger.warn(
        undefined,
        `refresh DingTalk stream failed: ${
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
