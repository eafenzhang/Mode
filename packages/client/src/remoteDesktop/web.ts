import type { WebBusinessStream, WebControlLink } from "@mode/remote-core/web";
import type {
  RemoteDesktopCodec,
  RemoteDesktopMediaSocket,
  RemoteDesktopQuality,
  RemoteDesktopSessionEvent,
  RemoteDesktopSignalMessage,
  RemoteDesktopStartSessionResult,
} from "@mode/shared/remote-desktop";

import {
  createRemoteDesktopHostController,
  type RemoteDesktopSignalSocket,
} from "./controller.js";

/**
 * 浏览器 v3 主控：会话创建走 `remote.http.request`，信令/媒体走 `remote.ws` 隧道。
 *
 * 和桌面 Host 的差别只有两处：没有本机回环代理，所以地址是虚拟 URL（host 固定为
 * `v3-remote.invalid`），由 `openSignalSocket` / `openMediaSocket` 把路径+票据还原成
 * 隧道；其余状态机完全复用 `controller.ts` 的唯一实现。
 */
const VIRTUAL_HOST = "wss://v3-remote.invalid";

type Logger = { info(...args: unknown[]): void; warn(...args: unknown[]): void };

export interface CreateWebRemoteDesktopSessionParams {
  link: Pick<WebControlLink, "request" | "openStream">;
  hostDeviceId: string;
  quality?: RemoteDesktopQuality;
  codec?: RemoteDesktopCodec;
  /** 会话事件（state/signal），与桌面 `onRemoteDesktopSessionEvent` 同口径。 */
  onEvent: (event: RemoteDesktopSessionEvent) => void;
  logger?: Logger;
  /** 会话创建请求超时；被控端同意窗最长 60s，默认 75s。 */
  timeoutMs?: number;
}

export interface WebRemoteDesktopSession {
  start(): Promise<RemoteDesktopStartSessionResult>;
  sendSignal(message: RemoteDesktopSignalMessage): void;
  end(reason?: "user-end" | "error"): Promise<void>;
  dispose(): Promise<void>;
  /** 把 `start()` 返回的 mediaUrl 接到 E2EE 隧道；作为 `createNativeMediaSession` 的 openSocket。 */
  openMediaSocket(url: string): RemoteDesktopMediaSocket;
  /** 浏览器剪贴板（需用户手势授权的环境下由调用方保证时机）。 */
  readClipboardText(): Promise<string>;
}

function decodeBase64UrlSafe(bodyBase64: string): Uint8Array {
  // HTTP 桥用 base64url；atob 只认标准字母表，先归一化。
  const normalized = bodyBase64.replace(/-/gu, "+").replace(/_/gu, "/");
  const padded = normalized.padEnd(Math.ceil(normalized.length / 4) * 4, "=");
  const binary = atob(padded);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes;
}

function tunnelPath(url: string): string {
  const parsed = new URL(url);
  return `${parsed.pathname}${parsed.search}`;
}

function encodeTextFrame(text: string): Uint8Array {
  const payload = new TextEncoder().encode(text);
  const frame = new Uint8Array(payload.length + 1);
  frame[0] = 0;
  frame.set(payload, 1);
  return frame;
}

/** 隧道帧 → 文本/二进制；空帧（流刚打开）忽略。 */
function decodeTunnelFrame(frame: Uint8Array): { kind: 0 | 1; payload: Uint8Array } | null {
  if (frame.length === 0) return null;
  return { kind: frame[0] === 1 ? 1 : 0, payload: frame.subarray(1) };
}

function createSignalSocket(stream: WebBusinessStream): RemoteDesktopSignalSocket {
  const listeners = {
    open: new Set<() => void>(),
    message: new Set<(data: unknown) => void>(),
    close: new Set<() => void>(),
    error: new Set<(error: Error) => void>(),
  };
  let readyState = 0;
  let closed = false;
  const emitClose = (): void => {
    if (closed) return;
    closed = true;
    readyState = 3;
    for (const listener of listeners.close) listener();
  };

  stream.onData((frame) => {
    const decoded = decodeTunnelFrame(frame);
    if (!decoded) return;
    const data =
      decoded.kind === 0
        ? new TextDecoder().decode(decoded.payload)
        : decoded.payload.buffer.slice(
            decoded.payload.byteOffset,
            decoded.payload.byteOffset + decoded.payload.byteLength,
          );
    for (const listener of listeners.message) listener(data);
  });
  stream.onClose(emitClose);
  // 隧道 attach 为异步：先按 CONNECTING，下一拍声明 open（上行帧由 Host 按 socket open 排队）。
  queueMicrotask(() => {
    if (closed) return;
    readyState = 1;
    for (const listener of listeners.open) listener();
  });

  return {
    get readyState() {
      return readyState;
    },
    on(event, listener) {
      listeners[event].add(listener as never);
    },
    send(text) {
      if (closed) return;
      stream.send(encodeTextFrame(text));
    },
    close() {
      if (closed) return;
      try {
        stream.close("client ended");
      } finally {
        emitClose();
      }
    },
    removeAllListeners() {
      for (const set of Object.values(listeners)) set.clear();
    },
  };
}

function createMediaSocket(stream: WebBusinessStream): RemoteDesktopMediaSocket {
  let readyState = 0;
  let closed = false;
  const socket: RemoteDesktopMediaSocket = {
    get readyState() {
      return readyState;
    },
    binaryType: "arraybuffer",
    onopen: null,
    onmessage: null,
    onclose: null,
    onerror: null,
    send(data) {
      if (closed || readyState !== 1) return;
      if (typeof data === "string") {
        stream.send(encodeTextFrame(data));
        return;
      }
      const payload =
        data instanceof ArrayBuffer
          ? new Uint8Array(data)
          : ArrayBuffer.isView(data)
            ? new Uint8Array(data.buffer, data.byteOffset, data.byteLength)
            : new Uint8Array(data as ArrayBufferLike);
      const frame = new Uint8Array(payload.length + 1);
      frame[0] = 1;
      frame.set(payload, 1);
      stream.send(frame);
    },
    close() {
      if (closed) return;
      try {
        stream.close("client-stop");
      } finally {
        emitClose();
      }
    },
  };
  const emitClose = (): void => {
    if (closed) return;
    closed = true;
    readyState = 3;
    socket.onclose?.();
  };

  stream.onData((frame) => {
    const decoded = decodeTunnelFrame(frame);
    if (!decoded) return;
    const data =
      decoded.kind === 0
        ? new TextDecoder().decode(decoded.payload)
        : decoded.payload.buffer.slice(
            decoded.payload.byteOffset,
            decoded.payload.byteOffset + decoded.payload.byteLength,
          );
    socket.onmessage?.({ data });
  });
  stream.onClose(emitClose);
  queueMicrotask(() => {
    if (closed) return;
    readyState = 1;
    socket.onopen?.();
  });
  return socket;
}

export function createWebRemoteDesktopSession(
  params: CreateWebRemoteDesktopSessionParams,
): WebRemoteDesktopSession {
  const logger: Logger = params.logger ?? { info: () => undefined, warn: () => undefined };
  const timeoutMs = params.timeoutMs ?? 75_000;

  const openSocket = (url: string): RemoteDesktopSignalSocket => {
    const path = tunnelPath(url);
    if (!path.startsWith("/ws/remote-desktop")) {
      throw new Error(`不支持的信令地址：${url}`);
    }
    return createSignalSocket(params.link.openStream("remote.ws", { path }));
  };

  const controller = createRemoteDesktopHostController({
    openSession: async (_serverId, quality, codec) => {
      // 本机能力预检先于请求：缺 WebCodecs/WebCrypto（含非安全上下文）时不弹对端同意框。
      const hasVideoDecoder =
        typeof (globalThis as { VideoDecoder?: unknown }).VideoDecoder !== "undefined";
      if (!hasVideoDecoder || !globalThis.crypto?.subtle) {
        return {
          ok: false,
          errorCode: "decoder_unsupported",
          error: "浏览器缺少 WebCodecs/WebCrypto，需 HTTPS 与新版 Chrome/Edge",
        };
      }
      const query = new URLSearchParams();
      if (quality) query.set("quality", quality);
      if (codec) query.set("codec", codec);
      const suffix = query.size > 0 ? `?${query.toString()}` : "";
      const response = (await params.link.request(
        "remote.http.request",
        {
          method: "POST",
          path: `/api/remote-desktop/session${suffix}`,
        },
        timeoutMs,
      )) as { status?: unknown; bodyBase64?: unknown };

      if (response.status !== 200) {
        let code = `HTTP ${String(response.status)}`;
        if (typeof response.bodyBase64 === "string") {
          try {
            const body = JSON.parse(
              new TextDecoder().decode(decodeBase64UrlSafe(response.bodyBase64)),
            ) as { code?: unknown };
            if (typeof body.code === "string") code = body.code;
          } catch {
            // 错误体不可解析时保留 HTTP 状态。
          }
        }
        return { ok: false, errorCode: code, error: code };
      }
      if (typeof response.bodyBase64 !== "string") {
        return { ok: false, error: "对端响应缺少会话票据" };
      }
      let ticket: string | undefined;
      let mediaTicket: string | undefined;
      try {
        const body = JSON.parse(
          new TextDecoder().decode(decodeBase64UrlSafe(response.bodyBase64)),
        ) as { ticket?: unknown; mediaTicket?: unknown };
        if (typeof body.ticket === "string") ticket = body.ticket;
        if (typeof body.mediaTicket === "string") mediaTicket = body.mediaTicket;
      } catch {
        return { ok: false, error: "对端会话响应不可解析" };
      }
      if (!ticket || !mediaTicket) {
        return { ok: false, error: "对端响应缺少信令/媒体票据" };
      }
      return {
        ok: true,
        wsUrl: `${VIRTUAL_HOST}/ws/remote-desktop?ticket=${encodeURIComponent(ticket)}`,
        mediaUrl: `${VIRTUAL_HOST}/ws/remote-desktop-media?mediaTicket=${encodeURIComponent(mediaTicket)}`,
      };
    },
    openSocket,
    logger,
    push: params.onEvent,
  });

  return {
    start: () =>
      controller.start({
        serverId: params.hostDeviceId,
        ...(params.quality ? { quality: params.quality } : {}),
        ...(params.codec ? { codec: params.codec } : {}),
      }),
    sendSignal: (message) => controller.sendSignal(message),
    end: (reason) => controller.end(reason),
    dispose: () => controller.dispose(),
    openMediaSocket: (url) => {
      const path = tunnelPath(url);
      if (!path.startsWith("/ws/remote-desktop-media")) {
        throw new Error(`不支持的媒体地址：${url}`);
      }
      return createMediaSocket(params.link.openStream("remote.ws", { path }));
    },
    readClipboardText: async () => {
      // Node 的类型声明把 `navigator` 收窄成无 clipboard 的形态；浏览器里按标准 API 取。
      const clipboard = (
        globalThis as {
          navigator?: { clipboard?: { readText?(): Promise<string> } };
        }
      ).navigator?.clipboard;
      if (!clipboard?.readText) {
        throw new Error("当前浏览器不支持读取剪贴板");
      }
      return await clipboard.readText();
    },
  };
}
