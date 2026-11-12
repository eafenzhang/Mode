import type { RemoteSocket } from "../app/controlTypes.js";
import type { DirectAssignment } from "../domain/controlMessages.js";
import { openBrowserSocket, parseControlEnvelope, type WebSocketLike } from "./transport.js";

const DEFAULT_READY_TIMEOUT_MS = 3_000;

/** 地址 → 直连 URL：保留地址自带 scheme；纯 host:port 视为明文 ws。 */
export function buildWebDirectSocketUrl(address: string): string {
  const trimmed = address.trim().replace(/\/+$/u, "");
  const base = /^wss?:\/\//iu.test(trimmed) ? trimmed : `ws://${trimmed}`;
  return `${base}/ws/v1/direct`;
}

function isMixedContentBlocked(url: string): boolean {
  const protocol = (globalThis as { location?: { protocol?: string } }).location?.protocol;
  return protocol === "https:" && url.startsWith("ws://");
}

interface DirectSocketOptions {
  direct?: DirectAssignment;
  connectionId: string;
  createSocket?: (url: string) => Promise<WebSocketLike>;
  timeoutMs?: number;
  openSocket?: (url: string) => Promise<WebSocketLike>;
}

/**
 * 浏览器 LAN 直连：逐个尝试 Host 上报的地址，首帧 `direct.attach` 自证 ticket，
 * 收到 `relay.ready` 后返回数据 socket。全部失败返回 undefined，由调用方回落 Relay。
 *
 * HTTPS 页面不能连 `ws://`（混合内容），这类地址直接跳过；Host 配置受信任 TLS 后
 * 地址会带 `wss://` 前缀。
 */
export async function openWebDirectSocket(
  options: DirectSocketOptions,
): Promise<RemoteSocket | undefined> {
  const direct = options.direct;
  if (!direct || direct.addresses.length === 0) return undefined;
  const openSocket = options.openSocket ?? options.createSocket ?? openBrowserSocket;
  const timeoutMs = options.timeoutMs ?? DEFAULT_READY_TIMEOUT_MS;
  for (const address of direct.addresses) {
    const url = buildWebDirectSocketUrl(address);
    if (isMixedContentBlocked(url)) continue;
    try {
      return await openOne(url, direct.ticket, options.connectionId, openSocket, timeoutMs);
    } catch {
      // 该地址失败：继续尝试下一个。
    }
  }
  return undefined;
}

function openOne(
  url: string,
  ticket: string,
  connectionId: string,
  openSocket: (url: string) => Promise<WebSocketLike>,
  timeoutMs: number,
): Promise<RemoteSocket> {
  return new Promise((resolve, reject) => {
    let ws: WebSocketLike | null = null;
    let settled = false;
    let ready = false;
    const buffered: string[] = [];
    let onMessage: ((text: string) => void) | null = null;
    const closeListeners = new Set<(code: number, reason: string) => void>();
    let closed = false;
    let closeCode = 1006;
    let closeReason = "direct socket closed";

    const fail = (error: Error): void => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      try {
        ws?.close(1000, "direct failed");
      } catch {
        // 已关闭时忽略。
      }
      reject(error);
    };
    const timer = setTimeout(() => fail(new Error("LAN direct ready timeout")), timeoutMs);

    const socket: RemoteSocket = {
      send(text) {
        if (ws && ws.readyState === 1) ws.send(text);
      },
      close(code = 1000, reason = "") {
        ws?.close(code, reason);
      },
      onMessage(callback) {
        onMessage = callback;
        for (const text of buffered.splice(0, buffered.length)) callback(text);
      },
      onClose(callback) {
        closeListeners.add(callback);
        if (closed) queueMicrotask(() => callback(closeCode, closeReason));
      },
    };

    void openSocket(url)
      .then((opened) => {
        if (settled) {
          opened.close(1000, "direct cancelled");
          return;
        }
        ws = opened;
        opened.addEventListener("message", (event) => {
          if (typeof event.data !== "string") return;
          const text = event.data;
          if (!ready) {
            const frame = parseControlEnvelope(text);
            if (frame?.type === "relay.ready" && frame.payload.connectionId === connectionId) {
              ready = true;
              settled = true;
              clearTimeout(timer);
              resolve(socket);
              return;
            }
          }
          if (ready) {
            onMessage?.(text);
          } else {
            buffered.push(text);
          }
        });
        opened.addEventListener("close", (event) => {
          closed = true;
          closeCode = event.code ?? 1006;
          closeReason = `direct ws closed ${closeCode}`;
          fail(new Error(closeReason));
          for (const listener of closeListeners) listener(closeCode, closeReason);
          closeListeners.clear();
        });
        opened.send(JSON.stringify({ type: "direct.attach", connectionId, ticket }));
      })
      .catch((error: unknown) => fail(error instanceof Error ? error : new Error(String(error))));
  });
}

/** connect.accepted.direct 的宽松解析：缺字段或空地址直接视为无直连候选。 */
export function readDirectAssignment(value: unknown): DirectAssignment | undefined {
  if (!value || typeof value !== "object") return undefined;
  const candidate = value as { ticket?: unknown; addresses?: unknown };
  if (typeof candidate.ticket !== "string") return undefined;
  if (!Array.isArray(candidate.addresses)) return undefined;
  const addresses = candidate.addresses.filter(
    (address): address is string => typeof address === "string" && address.length > 0,
  );
  return addresses.length > 0 ? { ticket: candidate.ticket, addresses } : undefined;
}
