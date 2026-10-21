/** 中继服务内部共享类型与 HTTP/WS 小工具。 */
import { createHash, timingSafeEqual } from "node:crypto";
import type { IncomingHttpHeaders, IncomingMessage, ServerResponse } from "node:http";
import type { WebSocket, RawData } from "ws";

import type {
  RelayHeaders,
  RelayHttpErrorControl,
  RelayHttpResponseControl,
  RelayWsErrorControl,
  RelayWsReadyControl,
} from "./protocol.js";

export interface PendingHttp {
  resolve: (message: RelayHttpResponseControl | RelayHttpErrorControl) => void;
  timer: NodeJS.Timeout;
}

export interface PendingWs {
  resolve: (message: RelayWsReadyControl | RelayWsErrorControl) => void;
  timer: NodeJS.Timeout;
}

export interface TunnelConnection {
  tunnelId: string;
  ws: WebSocket;
  nextStreamId: number;
  pendingHttp: Map<number, PendingHttp>;
  pendingWs: Map<number, PendingWs>;
  activeWs: Map<number, WebSocket>;
  heartbeat: NodeJS.Timeout | null;
  helloTimeout: NodeJS.Timeout | null;
}

export function toBuffer(data: RawData): Buffer {
  if (Array.isArray(data)) {
    return Buffer.concat(data);
  }
  return Buffer.isBuffer(data) ? data : Buffer.from(data as ArrayBuffer);
}

export function safeEqual(left: string, right: string): boolean {
  const leftHash = createHash("sha256").update(left).digest();
  const rightHash = createHash("sha256").update(right).digest();
  return timingSafeEqual(leftHash, rightHash);
}

const HOP_BY_HOP_HEADERS = new Set([
  "connection",
  "content-length",
  "host",
  "keep-alive",
  "proxy-authenticate",
  "proxy-authorization",
  "te",
  "trailer",
  "transfer-encoding",
  "upgrade",
]);

export function normalizeHeaders(headers: IncomingHttpHeaders): RelayHeaders {
  const result: RelayHeaders = {};
  for (const [name, value] of Object.entries(headers)) {
    const lower = name.toLowerCase();
    if (
      value === undefined ||
      HOP_BY_HOP_HEADERS.has(lower) ||
      lower.startsWith("sec-websocket-")
    ) {
      continue;
    }
    result[lower] = Array.isArray(value) ? value : String(value);
  }
  return result;
}

export function writeUpgradeError(
  socket: NodeJS.WritableStream,
  status: number,
  message: string,
): void {
  socket.write(`HTTP/1.1 ${status} ${message}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n`);
  socket.end();
}

export function allocateStreamId(tunnel: TunnelConnection): number {
  const streamId = tunnel.nextStreamId;
  tunnel.nextStreamId = (tunnel.nextStreamId + 1) >>> 0;
  if (tunnel.nextStreamId === 0) {
    tunnel.nextStreamId = 1;
  }
  return streamId;
}

export function readBody(req: IncomingMessage, maxBytes: number): Promise<Buffer> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    let size = 0;
    let settled = false;
    req.on("data", (chunk: Buffer) => {
      if (settled) {
        return;
      }
      size += chunk.length;
      if (size > maxBytes) {
        settled = true;
        req.pause();
        reject(new Error("body too large"));
        return;
      }
      chunks.push(chunk);
    });
    req.on("end", () => {
      if (!settled) {
        settled = true;
        resolve(Buffer.concat(chunks));
      }
    });
    req.on("error", (error) => {
      if (!settled) {
        settled = true;
        reject(error);
      }
    });
  });
}

export function writeHttpResponse(res: ServerResponse, response: RelayHttpResponseControl): void {
  const headers: RelayHeaders = {};
  for (const [name, value] of Object.entries(response.headers)) {
    const lower = name.toLowerCase();
    if (lower === "connection" || lower === "content-length" || lower === "transfer-encoding") {
      continue;
    }
    headers[lower] = value;
  }
  const body = response.bodyBase64 ? Buffer.from(response.bodyBase64, "base64") : Buffer.alloc(0);
  headers["content-length"] = String(body.length);
  res.writeHead(response.status, headers);
  res.end(body);
}
