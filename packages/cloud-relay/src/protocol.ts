/**
 * WSS 中继隧道协议（docs/specs/wss-relay-mvp.md）。
 *
 * 隧道是一条 WSS：控制消息走 JSON text frame；WebSocket 数据走 binary frame
 * `[u32 streamId][u8 kind][payload]`，kind 1=text、2=binary。HTTP 消息体用 base64
 * 放在 JSON 里（远控 API 都是小包；大流量只走 WS 数据帧）。
 */

export const RELAY_PROTOCOL_VERSION = 1;
export const RELAY_TUNNEL_PATH = "/_tunnel";
export const RELAY_PUBLIC_PREFIX = "/d/";
export const RELAY_TUNNEL_ID_PATTERN = /^[A-Za-z0-9_-]{16,128}$/u;
export const RELAY_MAX_HTTP_BODY_BYTES = 4 * 1024 * 1024;
export const RELAY_MAX_WS_FRAME_BYTES = 16 * 1024 * 1024;
export const RELAY_HEARTBEAT_MS = 30_000;
export const RELAY_HTTP_TIMEOUT_MS = 120_000;
export const RELAY_WS_OPEN_TIMEOUT_MS = 15_000;

export type RelayHeaders = Record<string, string | string[]>;

export interface RelayHelloControl {
  type: "hello";
  version: number;
  tunnelId: string;
}

export interface RelayReadyControl {
  type: "ready";
  version: number;
  publicPath: string;
}

export interface RelayPingControl {
  type: "ping" | "pong";
  ts: number;
}

export interface RelayHttpRequestControl {
  type: "http-request";
  streamId: number;
  method: string;
  path: string;
  headers: RelayHeaders;
  bodyBase64?: string;
}

export interface RelayHttpResponseControl {
  type: "http-response";
  streamId: number;
  status: number;
  headers: RelayHeaders;
  bodyBase64?: string;
}

export interface RelayHttpErrorControl {
  type: "http-error";
  streamId: number;
  message: string;
}

export interface RelayWsOpenControl {
  type: "ws-open";
  streamId: number;
  path: string;
  headers: RelayHeaders;
}

export interface RelayWsReadyControl {
  type: "ws-ready";
  streamId: number;
}

export interface RelayWsErrorControl {
  type: "ws-error";
  streamId: number;
  message: string;
}

export interface RelayWsCloseControl {
  type: "ws-close";
  streamId: number;
  code?: number;
  reason?: string;
}

export type RelayTunnelControl =
  | RelayHelloControl
  | RelayReadyControl
  | RelayPingControl
  | RelayHttpRequestControl
  | RelayHttpResponseControl
  | RelayHttpErrorControl
  | RelayWsOpenControl
  | RelayWsReadyControl
  | RelayWsErrorControl
  | RelayWsCloseControl;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isFiniteNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value);
}

function isHeaders(value: unknown): value is RelayHeaders {
  if (!isRecord(value)) {
    return false;
  }
  for (const header of Object.values(value)) {
    if (typeof header !== "string" && !Array.isArray(header)) {
      return false;
    }
    if (Array.isArray(header) && header.some((item) => typeof item !== "string")) {
      return false;
    }
  }
  return true;
}

/** 解析一条控制消息；未知/缺字段一律返回 null，由调用方关闭或忽略。 */
export function parseTunnelControl(text: string): RelayTunnelControl | null {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return null;
  }
  if (!isRecord(value) || typeof value.type !== "string") {
    return null;
  }
  switch (value.type) {
    case "hello":
      return isFiniteNumber(value.version) && typeof value.tunnelId === "string"
        ? (value as unknown as RelayHelloControl)
        : null;
    case "ready":
      return isFiniteNumber(value.version) && typeof value.publicPath === "string"
        ? (value as unknown as RelayReadyControl)
        : null;
    case "ping":
    case "pong":
      return isFiniteNumber(value.ts) ? (value as unknown as RelayPingControl) : null;
    case "http-request":
      return isFiniteNumber(value.streamId) &&
        typeof value.method === "string" &&
        typeof value.path === "string" &&
        isHeaders(value.headers)
        ? (value as unknown as RelayHttpRequestControl)
        : null;
    case "http-response":
      return isFiniteNumber(value.streamId) &&
        isFiniteNumber(value.status) &&
        isHeaders(value.headers)
        ? (value as unknown as RelayHttpResponseControl)
        : null;
    case "http-error":
      return isFiniteNumber(value.streamId) && typeof value.message === "string"
        ? (value as unknown as RelayHttpErrorControl)
        : null;
    case "ws-open":
      return isFiniteNumber(value.streamId) &&
        typeof value.path === "string" &&
        isHeaders(value.headers)
        ? (value as unknown as RelayWsOpenControl)
        : null;
    case "ws-ready":
      return isFiniteNumber(value.streamId) ? (value as unknown as RelayWsReadyControl) : null;
    case "ws-error":
      return isFiniteNumber(value.streamId) && typeof value.message === "string"
        ? (value as unknown as RelayWsErrorControl)
        : null;
    case "ws-close":
      return isFiniteNumber(value.streamId) ? (value as unknown as RelayWsCloseControl) : null;
    default:
      return null;
  }
}

export interface RelayDataFrame {
  streamId: number;
  isBinary: boolean;
  payload: Buffer;
}

export function encodeTunnelDataFrame(
  streamId: number,
  isBinary: boolean,
  payload: Buffer,
): Buffer {
  const header = Buffer.allocUnsafe(5);
  header.writeUInt32BE(streamId >>> 0, 0);
  header.writeUInt8(isBinary ? 2 : 1, 4);
  return Buffer.concat([header, payload]);
}

export function decodeTunnelDataFrame(data: Buffer): RelayDataFrame | null {
  if (data.length < 5) {
    return null;
  }
  const streamId = data.readUInt32BE(0);
  const kind = data.readUInt8(4);
  if (kind !== 1 && kind !== 2) {
    return null;
  }
  return { streamId, isBinary: kind === 2, payload: data.subarray(5) };
}

/** 把 `/d/<tunnelId>/...` 拆成 tunnelId 与转发路径（保留尾部 `/`）。 */
export function parsePublicPath(pathname: string): { tunnelId: string; rest: string } | null {
  if (!pathname.startsWith(RELAY_PUBLIC_PREFIX)) {
    return null;
  }
  const remainder = pathname.slice(RELAY_PUBLIC_PREFIX.length);
  const slash = remainder.indexOf("/");
  const tunnelId = slash === -1 ? remainder : remainder.slice(0, slash);
  if (!RELAY_TUNNEL_ID_PATTERN.test(tunnelId)) {
    return null;
  }
  return { tunnelId, rest: slash === -1 ? "/" : remainder.slice(slash) };
}
