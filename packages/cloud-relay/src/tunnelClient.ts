/**
 * WSS 中继的出站隧道客户端（docs/specs/wss-relay-mvp.md）。
 *
 * 运行在被控端（或同局域网机器）：连中继的 `/_tunnel`，把 HTTP/WS 请求转发到
 * 本地 Mode Host（默认 http://127.0.0.1:45880）。不要求被控端有公网 IP。
 */
import { request as httpRequest } from "node:http";
import { request as httpsRequest } from "node:https";
import { WebSocket, type RawData } from "ws";

import {
  decodeTunnelDataFrame,
  encodeTunnelDataFrame,
  parseTunnelControl,
  RELAY_HTTP_TIMEOUT_MS,
  RELAY_MAX_WS_FRAME_BYTES,
  RELAY_PROTOCOL_VERSION,
  RELAY_TUNNEL_PATH,
  RELAY_WS_OPEN_TIMEOUT_MS,
  type RelayHeaders,
  type RelayHttpRequestControl,
  type RelayTunnelControl,
  type RelayWsOpenControl,
} from "./protocol.js";

export interface RelayTunnelClientOptions {
  /** 例如 wss://relay.example.com/_tunnel */
  relayUrl: string;
  token: string;
  tunnelId: string;
  /** 例如 http://127.0.0.1:45880 */
  localBaseUrl: string;
  log?: (line: string) => void;
  reconnect?: boolean;
}

export interface RelayTunnelClient {
  close(): void;
}

const MAX_LOCAL_RESPONSE_BYTES = 8 * 1024 * 1024;

function toBuffer(data: RawData): Buffer {
  if (Array.isArray(data)) {
    return Buffer.concat(data);
  }
  return Buffer.isBuffer(data) ? data : Buffer.from(data as ArrayBuffer);
}

function withTrailingSlash(value: string): string {
  return value.endsWith("/") ? value : `${value}/`;
}

function localUrlFor(base: string, path: string): URL {
  return new URL(path.startsWith("/") ? path.slice(1) : path, withTrailingSlash(base));
}

function normalizeResponseHeaders(
  headers: Record<string, string | string[] | undefined>,
): RelayHeaders {
  const result: RelayHeaders = {};
  for (const [name, value] of Object.entries(headers)) {
    if (value === undefined) {
      continue;
    }
    result[name.toLowerCase()] = value;
  }
  return result;
}

export function startRelayTunnelClient(options: RelayTunnelClientOptions): RelayTunnelClient {
  const log = options.log ?? ((line: string) => console.log(line));
  const localBaseUrl = options.localBaseUrl;
  const localSockets = new Map<number, WebSocket>();
  let tunnel: WebSocket | null = null;
  let closed = false;
  let attempt = 0;
  let reconnectTimer: NodeJS.Timeout | null = null;

  const sendText = (control: RelayTunnelControl): void => {
    if (!tunnel || tunnel.readyState !== WebSocket.OPEN) {
      return;
    }
    try {
      tunnel.send(JSON.stringify(control));
    } catch {
      // 连接正在关闭；close/error 会触发重连。
    }
  };

  const sendFrame = (streamId: number, isBinary: boolean, payload: Buffer): void => {
    if (
      !tunnel ||
      tunnel.readyState !== WebSocket.OPEN ||
      payload.length > RELAY_MAX_WS_FRAME_BYTES
    ) {
      return;
    }
    try {
      tunnel.send(encodeTunnelDataFrame(streamId, isBinary, payload));
    } catch {
      // 同上。
    }
  };

  const handleHttp = (control: RelayHttpRequestControl): void => {
    let localUrl: URL;
    try {
      localUrl = localUrlFor(localBaseUrl, control.path);
    } catch {
      sendText({ type: "http-error", streamId: control.streamId, message: "invalid local path" });
      return;
    }
    const body = control.bodyBase64 ? Buffer.from(control.bodyBase64, "base64") : Buffer.alloc(0);
    const requestFn = localUrl.protocol === "https:" ? httpsRequest : httpRequest;
    const request = requestFn(
      localUrl,
      { method: control.method, headers: control.headers },
      (response) => {
        const chunks: Buffer[] = [];
        let size = 0;
        response.on("data", (chunk: Buffer) => {
          size += chunk.length;
          if (size > MAX_LOCAL_RESPONSE_BYTES) {
            response.destroy(new Error("local response too large"));
            sendText({
              type: "http-error",
              streamId: control.streamId,
              message: "local response too large",
            });
            return;
          }
          chunks.push(chunk);
        });
        response.on("end", () => {
          const payload = Buffer.concat(chunks);
          sendText({
            type: "http-response",
            streamId: control.streamId,
            status: response.statusCode ?? 502,
            headers: normalizeResponseHeaders(response.headers),
            ...(payload.length > 0 ? { bodyBase64: payload.toString("base64") } : {}),
          });
        });
        response.on("error", (error) => {
          sendText({
            type: "http-error",
            streamId: control.streamId,
            message: error.message,
          });
        });
      },
    );
    request.setTimeout(RELAY_HTTP_TIMEOUT_MS, () => {
      request.destroy(new Error("local request timeout"));
    });
    request.on("error", (error) => {
      sendText({ type: "http-error", streamId: control.streamId, message: error.message });
    });
    request.end(body);
  };

  const handleWsOpen = (control: RelayWsOpenControl): void => {
    let localUrl: URL;
    try {
      localUrl = localUrlFor(localBaseUrl, control.path);
    } catch {
      sendText({ type: "ws-error", streamId: control.streamId, message: "invalid local path" });
      return;
    }
    localUrl.protocol = localUrl.protocol === "https:" ? "wss:" : "ws:";
    const ws = new WebSocket(localUrl, {
      headers: control.headers,
      handshakeTimeout: RELAY_WS_OPEN_TIMEOUT_MS,
    });
    localSockets.set(control.streamId, ws);
    ws.on("open", () => {
      sendText({ type: "ws-ready", streamId: control.streamId });
    });
    ws.on("message", (data, isBinary) => {
      sendFrame(control.streamId, isBinary, toBuffer(data));
    });
    ws.on("close", (code, reason) => {
      localSockets.delete(control.streamId);
      sendText({
        type: "ws-close",
        streamId: control.streamId,
        code,
        reason: reason.toString(),
      });
    });
    ws.on("error", (error) => {
      localSockets.delete(control.streamId);
      sendText({
        type: "ws-error",
        streamId: control.streamId,
        message: error.message,
      });
    });
  };

  const handleControl = (control: RelayTunnelControl): void => {
    switch (control.type) {
      case "ready":
        attempt = 0;
        log(`[relay-tunnel] online publicPath=${control.publicPath}`);
        return;
      case "ping":
        sendText({ type: "pong", ts: control.ts });
        return;
      case "http-request":
        handleHttp(control);
        return;
      case "ws-open":
        handleWsOpen(control);
        return;
      case "ws-close": {
        const ws = localSockets.get(control.streamId);
        localSockets.delete(control.streamId);
        ws?.close(control.code ?? 1000, control.reason ?? "");
        return;
      }
      default:
        return;
    }
  };

  const closeLocalSockets = (): void => {
    for (const ws of localSockets.values()) {
      try {
        ws.close(1011, "relay tunnel closed");
      } catch {
        // 已关闭。
      }
    }
    localSockets.clear();
  };

  const scheduleReconnect = (): void => {
    if (closed || options.reconnect === false) {
      return;
    }
    const delay = Math.min(30_000, 1_000 * 2 ** attempt);
    attempt += 1;
    log(`[relay-tunnel] reconnect in ${delay}ms`);
    reconnectTimer = setTimeout(connect, delay);
    reconnectTimer.unref?.();
  };

  const connect = (): void => {
    if (closed) {
      return;
    }
    let url: URL;
    try {
      url = new URL(options.relayUrl);
      url.searchParams.set("tunnelId", options.tunnelId);
      url.searchParams.set("token", options.token);
    } catch (error) {
      log(`[relay-tunnel] invalid relay url: ${String(error)}`);
      return;
    }
    const ws = new WebSocket(url, { handshakeTimeout: 10_000 });
    tunnel = ws;
    ws.on("open", () => {
      ws.send(
        JSON.stringify({
          type: "hello",
          version: RELAY_PROTOCOL_VERSION,
          tunnelId: options.tunnelId,
        }),
      );
    });
    ws.on("message", (data, isBinary) => {
      if (isBinary) {
        const frame = decodeTunnelDataFrame(toBuffer(data));
        if (!frame) {
          return;
        }
        const localWs = localSockets.get(frame.streamId);
        if (!localWs || localWs.readyState !== WebSocket.OPEN) {
          return;
        }
        localWs.send(frame.payload, { binary: frame.isBinary });
        return;
      }
      const control = parseTunnelControl(data.toString());
      if (control) {
        handleControl(control);
      }
    });
    ws.on("close", () => {
      if (tunnel === ws) {
        tunnel = null;
      }
      closeLocalSockets();
      scheduleReconnect();
    });
    ws.on("error", (error) => {
      log(`[relay-tunnel] socket error: ${error.message}`);
    });
  };

  connect();

  return {
    close() {
      closed = true;
      if (reconnectTimer) {
        clearTimeout(reconnectTimer);
        reconnectTimer = null;
      }
      closeLocalSockets();
      tunnel?.close(1000, "client shutdown");
      tunnel = null;
    },
  };
}

/** 供 CLI/部署文档复用：从 relay URL 推导公开基址。 */
export function buildRelayPublicBaseUrl(relayUrl: string, tunnelId: string): string {
  const url = new URL(relayUrl);
  const scheme = url.protocol === "wss:" ? "https:" : "http:";
  return `${scheme}//${url.host}/d/${tunnelId}`;
}

/** 默认隧道路径；CLI 用它校验/拼接 relay URL。 */
export const DEFAULT_RELAY_TUNNEL_PATH = RELAY_TUNNEL_PATH;
