import type { IncomingMessage } from "node:http";
import type { Duplex } from "node:stream";
import type { WebSocket } from "ws";

import type {
  ControlFrame,
  DeviceDescriptor,
  IceServerPayload,
  RelayAssignment,
  TransportKind,
} from "@mode/remote-core";

import type { CloudStore, ConnectionRow, DeviceRow } from "./store.js";

export class GatewayError extends Error {
  readonly status: number;
  readonly code: string;
  constructor(status: number, code: string, message: string) {
    super(message);
    this.name = "GatewayError";
    this.status = status;
    this.code = code;
  }
}

export interface ControlSocket {
  ws: WebSocket;
  device: DeviceRow;
  capabilities: string[];
  alive: boolean;
}

export interface ActiveConnection {
  row: ConnectionRow;
  transport: TransportKind | null;
  hostAssignment: RelayAssignment;
  clientAssignment: RelayAssignment;
  relayReady: Set<string>;
  relaySockets: Map<string, WebSocket>;
}

export interface ConnectionResult {
  connectionId: string;
  host: DeviceDescriptor;
  relay: RelayAssignment;
  direct?: { ticket: string; addresses: string[] };
  iceServers?: IceServerPayload[];
}

export interface GatewayMetrics {
  controlOpened(): void;
  controlClosed(): void;
  connectionOpened(): void;
  connectionClosed(): void;
  relayForwarded(bytes: number): void;
}

export interface GatewayOptions {
  store: CloudStore;
  secret: string;
  relayEndpoint: string | (() => string);
  nodeId?: string;
  allowedOrigins?: string[];
  /** WebRTC/P2P 选路的 ICE 配置；未配置时 connect.* 不带 iceServers。 */
  webrtc?: WebRtcConfig;
  metrics?: GatewayMetrics;
  log?: (line: string) => void;
}

export interface WebRtcTurnConfig {
  urls: string[];
  secret: string;
  ttlSeconds?: number;
}

export interface WebRtcConfig {
  stunUrls?: string[];
  turn?: WebRtcTurnConfig;
}

export function descriptorOf(device: DeviceRow): DeviceDescriptor {
  const descriptor: DeviceDescriptor = {
    deviceId: device.id,
    name: device.name,
    role: device.role,
    platform: device.platform,
    identityKey: device.identity_key,
  };
  if (device.client_version) descriptor.clientVersion = device.client_version;
  if (device.host_version) descriptor.hostVersion = device.host_version;
  return descriptor;
}

export function parseAddresses(value: string | null): string[] {
  if (!value) return [];
  try {
    const parsed = JSON.parse(value) as unknown;
    return Array.isArray(parsed)
      ? parsed.filter((entry): entry is string => typeof entry === "string")
      : [];
  } catch {
    return [];
  }
}

export function toBytes(data: unknown): Uint8Array {
  if (Buffer.isBuffer(data)) return data;
  if (data instanceof ArrayBuffer) return new Uint8Array(data);
  if (Array.isArray(data)) return Buffer.concat(data);
  return Buffer.from(String(data));
}

/** 浏览器 Origin 白名单；原生客户端不带 Origin，不在此拦截。 */
export function rejectDisallowedOrigin(
  req: IncomingMessage,
  socket: Duplex,
  allowedOrigins: string[] | undefined,
): boolean {
  const origin = req.headers.origin;
  if (!allowedOrigins?.length || !origin || allowedOrigins.includes(origin)) return false;
  socket.write("HTTP/1.1 403 Forbidden\r\nConnection: close\r\n\r\n");
  socket.destroy();
  return true;
}

export const SERVER_CAPABILITIES = [
  "secure.noise-ik.v1",
  "transport.lan",
  "transport.relay",
];

export function peerDeviceId(active: ActiveConnection, deviceId: string): string | null {
  if (active.row.host_device_id === deviceId) return active.row.client_device_id;
  if (active.row.client_device_id === deviceId) return active.row.host_device_id;
  return null;
}

export function errorFrame(error: unknown): ControlFrame {
  const normalized =
    error instanceof GatewayError
      ? error
      : new GatewayError(500, "internal", error instanceof Error ? error.message : String(error));
  return {
    v: 1,
    id: crypto.randomUUID(),
    timestamp: Date.now(),
    type: "error",
    payload: {
      code: normalized.code,
      message: normalized.message,
      retryable: normalized.status >= 500,
    },
  };
}

export type { ControlFrame };
