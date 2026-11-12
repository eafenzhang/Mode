import type { ControlFrame, RelayAssignment } from "@mode/remote-core";

import { buildIceServers } from "./webrtc.js";
import {
  GatewayError,
  descriptorOf,
  parseAddresses,
  type ActiveConnection,
  type ConnectionResult,
  type ControlSocket,
  type GatewayOptions,
} from "./gatewayTypes.js";
import type { DeviceRow } from "./store.js";
import { buildDirectAssignment, createRelayTicket } from "./tickets.js";

export interface GatewayConnectionContext {
  options: GatewayOptions;
  connections: Map<string, ActiveConnection>;
  controlSockets: Map<string, ControlSocket>;
  send(deviceId: string, frame: { type: ControlFrame["type"]; payload: unknown }): void;
  closeConnection(connectionId: string, code: number, reason: string): void;
}

/** 由 REST /api/v1/connections 与 Control connect.request 共用。 */
export function openConnectionFor(
  ctx: GatewayConnectionContext,
  client: DeviceRow,
  hostDeviceId: string,
  p2pPreferred: boolean,
): ConnectionResult {
  if (client.role !== "client") {
    throw new GatewayError(403, "role_required", "只有 Client 可以发起连接");
  }
  if (client.revoked_at !== null) {
    throw new GatewayError(403, "device_revoked", "Client 已撤销");
  }
  const host = ctx.options.store.findDevice(hostDeviceId);
  if (!host || host.revoked_at !== null) {
    throw new GatewayError(404, "host_not_found", "Host 不存在或已撤销");
  }
  if (host.role !== "host") throw new GatewayError(400, "invalid_host", "目标不是 Host");
  if (host.account_id !== client.account_id) {
    throw new GatewayError(403, "cross_account", "不允许跨账号连接");
  }
  if (!ctx.options.store.hasMembership(client.id, host.id)) {
    throw new GatewayError(403, "membership_required", "设备对没有有效授权");
  }
  const hostSocket = ctx.controlSockets.get(host.id);
  if (!hostSocket || hostSocket.ws.readyState !== WebSocket.OPEN) {
    throw new GatewayError(409, "host_offline", "Host 不在线");
  }
  // 同一 Client 的新连接替换旧连接；其他 Client 到同一 Host 的连接不受影响。
  for (const existing of ctx.options.store.listOpenConnections(client.id)) {
    if (existing.client_device_id === client.id) {
      ctx.closeConnection(existing.id, 4003, "connection replaced");
    }
  }
  const row = ctx.options.store.createConnection(client.id, host.id);
  const nodeId = ctx.options.nodeId ?? "relay-1";
  const epoch = 1;
  const endpoint =
    typeof ctx.options.relayEndpoint === "function"
      ? ctx.options.relayEndpoint()
      : ctx.options.relayEndpoint;
  const hostAssignment: RelayAssignment = {
    endpoint,
    ticket: createRelayTicket(ctx.options.secret, {
      connectionId: row.id,
      deviceId: host.id,
      peerDeviceId: client.id,
      nodeId,
      epoch,
    }),
    nodeId,
    epoch,
  };
  const clientAssignment: RelayAssignment = {
    endpoint,
    ticket: createRelayTicket(ctx.options.secret, {
      connectionId: row.id,
      deviceId: client.id,
      peerDeviceId: host.id,
      nodeId,
      epoch,
    }),
    nodeId,
    epoch,
  };
  ctx.connections.set(row.id, {
    row,
    transport: null,
    hostAssignment,
    clientAssignment,
    relayReady: new Set(),
    relaySockets: new Map(),
  });
  ctx.options.metrics?.connectionOpened();
  const direct = buildDirectAssignment(
    ctx.options.secret,
    { connectionId: row.id, hostDeviceId: host.id, clientDeviceId: client.id },
    parseAddresses(host.addresses),
  );
  const hostIceServers =
    ctx.options.iceServers?.(host.id) ?? buildIceServers(ctx.options.webrtc, host.id);
  const clientIceServers =
    ctx.options.iceServers?.(client.id) ?? buildIceServers(ctx.options.webrtc, client.id);
  ctx.send(host.id, {
    type: "connect.incoming",
    payload: {
      connectionId: row.id,
      client: descriptorOf(client),
      authorization: "account",
      relay: hostAssignment,
      ...(direct ? { direct } : {}),
      ...(p2pPreferred ? { p2pPreferred: true } : {}),
      ...(hostIceServers ? { iceServers: hostIceServers } : {}),
    },
  });
  return {
    connectionId: row.id,
    host: descriptorOf(host),
    relay: clientAssignment,
    ...(direct ? { direct } : {}),
    ...(p2pPreferred ? { p2pPreferred: true } : {}),
    ...(clientIceServers ? { iceServers: clientIceServers } : {}),
  };
}
