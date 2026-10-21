/**
 * 云端微信 Bot 登录（docs/specs/mode-cloud-account-directory.md）。
 *
 * 只做一件事：轮询 iLink 消息，把「允许的微信用户」发来的 6 位登录码标记为
 * approved。桌面端随后轮询 `/v1/auth/poll` 换取账号/设备/中继令牌。
 *
 * 不复用桌面 Bot 的 token：两个进程同时 getupdates 会互相抢消息。云端 Bot 必须是
 * 独立凭证（MODE_CLOUD_WECHAT_TOKEN）。
 */
import { createHash, createHmac, randomInt } from "node:crypto";

import type { CloudStore } from "./cloudStore.js";

const DEFAULT_ILINK_BASE_URL = "https://ilinkai.weixin.qq.com";
const CHANNEL_VERSION = "2.0.0";
const POLL_TIMEOUT_MS = 40_000;
const DEFAULT_POLL_INTERVAL_MS = 1_000;
const DEFAULT_RETRY_DELAY_MS = 5_000;

export interface CloudBotOptions {
  store: CloudStore;
  token: string;
  accountPepper: string;
  /** 允许登录的微信用户哈希；缺省 = TOFU，第一个提交有效登录码的用户绑定。 */
  allowedUserHash?: string;
  baseUrl?: string;
  pollIntervalMs?: number;
  retryDelayMs?: number;
  fetchImpl?: typeof fetch;
  now?: () => number;
  log: (line: string) => void;
}

export interface CloudBotHandle {
  close(): void;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function readString(record: Record<string, unknown> | null, key: string): string {
  const value = record?.[key];
  return typeof value === "string" ? value.trim() : "";
}

function unwrapData(payload: unknown): Record<string, unknown> {
  if (isRecord(payload) && isRecord(payload.data)) {
    return payload.data;
  }
  return isRecord(payload) ? payload : {};
}

function readMessages(payload: unknown): Record<string, unknown>[] {
  const container = unwrapData(payload);
  const raw = container.msgs ?? container.messages ?? container.updates ?? container.items ?? container.list;
  if (Array.isArray(raw)) {
    return raw.filter(isRecord);
  }
  return isRecord(raw) ? [raw] : [];
}

function readNextBuf(payload: unknown): string {
  const container = unwrapData(payload);
  return (
    readString(container, "get_updates_buf") ||
    readString(container, "buf") ||
    readString(container, "next_buf") ||
    readString(container, "nextBuf") ||
    readString(container, "getUpdatesBuf") ||
    readString(container, "syncKey")
  );
}

function readUserId(message: Record<string, unknown>): string {
  const from = isRecord(message.from) ? message.from : null;
  const sender = isRecord(message.sender) ? message.sender : null;
  return (
    readString(message, "from_user_id") ||
    readString(message, "from") ||
    readString(message, "from_user") ||
    readString(message, "fromUser") ||
    readString(message, "user") ||
    readString(message, "user_id") ||
    readString(message, "userId") ||
    readString(from, "id") ||
    readString(from, "wxid") ||
    readString(sender, "id") ||
    readString(sender, "wxid")
  );
}

function readTextItem(item: unknown): string {
  if (!isRecord(item)) {
    return "";
  }
  const textItem = isRecord(item.text_item) ? item.text_item : null;
  return readString(textItem, "text") || readString(item, "text") || readString(item, "content");
}

function readText(message: Record<string, unknown>): string {
  const direct =
    readString(message, "text") || readString(message, "content") || readString(message, "message");
  if (direct) {
    return direct;
  }
  const inner = isRecord(message.msg)
    ? message.msg
    : isRecord(message.message)
      ? message.message
      : null;
  const itemList = Array.isArray(message.item_list)
    ? message.item_list
    : Array.isArray(inner?.item_list)
      ? inner.item_list
      : [];
  return (
    itemList.map(readTextItem).filter(Boolean).join("\n") ||
    readString(inner, "text") ||
    readString(inner, "content")
  );
}

function buildHeaders(token: string): Record<string, string> {
  return {
    "content-type": "application/json",
    AuthorizationType: "ilink_bot_token",
    Authorization: `Bearer ${token}`,
    "X-WECHAT-UIN": Buffer.from(String(randomInt(0, 0x1_0000_0000)), "utf8").toString("base64"),
  };
}

function hashCode(code: string): string {
  return createHash("sha256").update(code).digest("hex");
}

function hashUser(pepper: string, userId: string): string {
  return createHmac("sha256", pepper).update(`weixin:${userId}`).digest("hex");
}

export function startCloudBot(options: CloudBotOptions): CloudBotHandle {
  const baseUrl = (options.baseUrl ?? DEFAULT_ILINK_BASE_URL).replace(/\/+$/u, "");
  const pollInterval = options.pollIntervalMs ?? DEFAULT_POLL_INTERVAL_MS;
  const retryDelay = options.retryDelayMs ?? DEFAULT_RETRY_DELAY_MS;
  const fetchImpl = options.fetchImpl ?? fetch;
  const now = options.now ?? (() => Date.now());
  let closed = false;
  let timer: NodeJS.Timeout | null = null;
  let buf = "";

  const handleMessage = (message: Record<string, unknown>): void => {
    const userId = readUserId(message);
    const text = readText(message);
    if (!userId || !text) {
      return;
    }
    const providerUserHash = hashUser(options.accountPepper, userId);
    const bound = options.store.getBotBinding();
    if (options.allowedUserHash && options.allowedUserHash !== providerUserHash) {
      return;
    }
    if (bound && bound !== providerUserHash) {
      return;
    }
    const match = /(?:^|\s)(\d{6})(?:\s|$)/u.exec(text);
    if (!match) {
      return;
    }
    const codeHash = hashCode(match[1]!);
    const code = options.store.getLoginCode(codeHash);
    const timestamp = now();
    if (!code || code.status !== "pending" || code.expiresAt <= timestamp) {
      return;
    }
    if (!options.store.approveLoginCode({ codeHash, providerUserHash, now: timestamp })) {
      return;
    }
    if (!bound) {
      // TOFU：第一个用有效登录码完成验证的微信用户成为账号锚点。
      options.store.bindBotUser(providerUserHash, timestamp);
      options.log("cloud_bot_binding_created");
    }
    options.log(`cloud_login_approved user=${providerUserHash.slice(0, 8)}…`);
  };

  const schedule = (delay: number): void => {
    if (closed) {
      return;
    }
    timer = setTimeout(() => void poll(), delay);
    timer.unref?.();
  };

  const poll = async (): Promise<void> => {
    if (closed) {
      return;
    }
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), POLL_TIMEOUT_MS);
    try {
      const response = await fetchImpl(`${baseUrl}/ilink/bot/getupdates`, {
        method: "POST",
        headers: buildHeaders(options.token),
        body: JSON.stringify({
          base_info: { channel_version: CHANNEL_VERSION },
          get_updates_buf: buf,
        }),
        signal: controller.signal,
      });
      if (!response.ok) {
        throw new Error(`HTTP ${response.status}`);
      }
      const payload = (await response.json()) as unknown;
      if (closed) {
        return;
      }
      for (const message of readMessages(payload)) {
        handleMessage(message);
      }
      const next = readNextBuf(payload);
      if (next) {
        buf = next;
      }
      schedule(pollInterval);
    } catch (error) {
      if (!closed) {
        const message = error instanceof Error ? error.message : String(error);
        options.log(`cloud_bot_poll_failed ${message}`);
        schedule(retryDelay);
      }
    } finally {
      clearTimeout(timeout);
    }
  };

  options.log("cloud_bot_started");
  void poll();

  return {
    close() {
      closed = true;
      if (timer) {
        clearTimeout(timer);
        timer = null;
      }
    },
  };
}
