import { randomUUID } from "node:crypto";
import type { IncomingMessage } from "node:http";
import type { Duplex } from "node:stream";
import { WebSocket, WebSocketServer } from "ws";

import {
  HEARTBEAT_INTERVAL_MS,
  HELLO_TIMEOUT_MS,
  decodeControlFrame,
  encodeControlFrame,
  type ControlFrame,
  type HelloPayload,
  type RelayAssignment,
} from "@mode/remote-core";

import {
  GatewayError,
  SERVER_CAPABILITIES,
  descriptorOf,
  errorFrame,
  parseAddresses,
  peerDeviceId,
  rejectDisallowedOrigin,
  toBytes,
  type ActiveConnection,
  type ConnectionResult,
  type ControlSocket,
  type GatewayOptions,
} from "./gatewayTypes.js";
import { handleRelaySocket } from "./relayChannel.js";
import { buildDirectAssignment, createRelayTicket } from "./tickets.js";
import type { CloudStore, DeviceRow } from "./store.js";

export { GatewayError, descriptorOf, parseAddresses } from "./gatewayTypes.js";

/** Control/Relay 网关：只做鉴权、路由和密文转发，不解析业务内容。 */
export class RemoteGateway {
  private readonly options: GatewayOptions;
  private readonly controlSockets = new Map<string, ControlSocket>();
  private readonly connections = new Map<string, ActiveConnection>();
  private readonly controlWss = new WebSocketServer({ noServer: true });
  private readonly relayWss = new WebSocketServer({ noServer: true });

  constructor(options: GatewayOptions) {
    this.options = options;
  }

  handleUpgrade(req: IncomingMessage, socket: Duplex, head: Buffer, url: URL): boolean {
    if (rejectDisallowedOrigin(req, socket, this.options.allowedOrigins)) return true;
    if (url.pathname === "/ws/v1/connect") {
      this.controlWss.handleUpgrade(req, socket, head, (ws) => this.acceptControl(ws));
      return true;
    }
    if (url.pathname === "/ws/v1/relay") {
      this.relayWss.handleUpgrade(req, socket, head, (ws) =>
        handleRelaySocket(ws, {
          secret: this.options.secret,
          connections: this.connections,
          sendFrame: (target, frame) => this.sendFrame(target, frame),
          closeConnection: (id, code, reason) => this.closeConnection(id, code, reason),
          onRelayForward: (bytes) => this.options.metrics?.relayForwarded(bytes),
        }),
      );
      return true;
    }
    return false;
  }

  isOnline(deviceId: string): boolean {
    const socket = this.controlSockets.get(deviceId);
    return Boolean(socket && socket.ws.readyState === WebSocket.OPEN);
  }

  /** 由 REST /api/v1/connections 与 Control connect.request 共用。 */
  openConnection(client: DeviceRow, hostDeviceId: string): ConnectionResult {
    if (client.role !== "client") {
      throw new GatewayError(403, "role_required", "只有 Client 可以发起连接");
    }
    if (client.revoked_at !== null) {
      throw new GatewayError(403, "device_revoked", "Client 已撤销");
    }
    const host = this.options.store.findDevice(hostDeviceId);
    if (!host || host.revoked_at !== null) {
      throw new GatewayError(404, "host_not_found", "Host 不存在或已撤销");
    }
    if (host.role !== "host") throw new GatewayError(400, "invalid_host", "目标不是 Host");
    if (host.account_id !== client.account_id) {
      throw new GatewayError(403, "cross_account", "不允许跨账号连接");
    }
    if (!this.options.store.hasMembership(client.id, host.id)) {
      throw new GatewayError(403, "membership_required", "设备对没有有效授权");
    }
    const hostSocket = this.controlSockets.get(host.id);
    if (!hostSocket || hostSocket.ws.readyState !== WebSocket.OPEN) {
      throw new GatewayError(409, "host_offline", "Host 不在线");
    }
    // 同一 Client 的新连接替换旧连接；其他 Client 到同一 Host 的连接不受影响。
    for (const existing of this.options.store.listOpenConnections(client.id)) {
      if (existing.client_device_id === client.id) {
        this.closeConnection(existing.id, 4003, "connection replaced");
      }
    }
    const row = this.options.store.createConnection(client.id, host.id);
    const nodeId = this.options.nodeId ?? "relay-1";
    const epoch = 1;
    const endpoint =
      typeof this.options.relayEndpoint === "function"
        ? this.options.relayEndpoint()
        : this.options.relayEndpoint;
    const hostAssignment: RelayAssignment = {
      endpoint,
      ticket: createRelayTicket(this.options.secret, {
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
      ticket: createRelayTicket(this.options.secret, {
        connectionId: row.id,
        deviceId: client.id,
        peerDeviceId: host.id,
        nodeId,
        epoch,
      }),
      nodeId,
      epoch,
    };
    this.connections.set(row.id, {
      row,
      transport: null,
      hostAssignment,
      clientAssignment,
      relayReady: new Set(),
      relaySockets: new Map(),
    });
    this.options.metrics?.connectionOpened();
    const direct = buildDirectAssignment(
      this.options.secret,
      { connectionId: row.id, hostDeviceId: host.id, clientDeviceId: client.id },
      parseAddresses(host.addresses),
    );
    this.send(host.id, {
      type: "connect.incoming",
      payload: {
        connectionId: row.id,
        client: descriptorOf(client),
        authorization: "account",
        relay: hostAssignment,
        ...(direct ? { direct } : {}),
      },
    });
    return {
      connectionId: row.id,
      host: descriptorOf(host),
      relay: clientAssignment,
      ...(direct ? { direct } : {}),
    };
  }

  closeConnection(connectionId: string, code = 4001, reason = "connection closed"): void {
    const active = this.connections.get(connectionId);
    this.options.store.closeConnection(connectionId, code);
    if (!active) return;
    this.connections.delete(connectionId);
    this.options.metrics?.connectionClosed();
    for (const [deviceId, socket] of active.relaySockets) {
      if (socket.readyState === WebSocket.OPEN) socket.close(code === 4004 ? 1000 : 1011, reason);
      active.relaySockets.delete(deviceId);
    }
    for (const deviceId of [active.row.host_device_id, active.row.client_device_id]) {
      this.send(deviceId, {
        type: "error",
        payload: { code: "CONNECTION_FAILED", message: reason, connectionId, retryable: code === 4001 },
      });
    }
  }

  closeDevice(deviceId: string, code = 4004): void {
    for (const [connectionId, active] of this.connections) {
      if (active.row.host_device_id === deviceId || active.row.client_device_id === deviceId) {
        this.closeConnection(connectionId, code, "device revoked");
      }
    }
    const socket = this.controlSockets.get(deviceId);
    if (socket) {
      socket.ws.close(code, "device revoked");
      this.controlSockets.delete(deviceId);
    }
  }

  close(): void {
    for (const connectionId of [...this.connections.keys()]) {
      this.closeConnection(connectionId, 4009, "server shutdown");
    }
    for (const socket of this.controlSockets.values()) socket.ws.close(4009, "server shutdown");
    this.controlSockets.clear();
    this.controlWss.close();
    this.relayWss.close();
  }
  private acceptControl(ws: WebSocket): void {
    this.options.metrics?.controlOpened();
    let authenticated = false;
    const helloTimer = setTimeout(() => {
      if (!authenticated) ws.close(4008, "hello timeout");
    }, HELLO_TIMEOUT_MS);
    ws.on("message", (data, isBinary) => {
      if (isBinary) {
        ws.close(4008, "binary control frame");
        return;
      }
      let frame: ControlFrame;
      try {
        frame = decodeControlFrame(toBytes(data));
      } catch (error) {
        this.options.log?.(
          `invalid control frame: ${error instanceof Error ? error.message : String(error)}`,
        );
        ws.close(4008, "invalid control frame");
        return;
      }
      if (!authenticated) {
        if (frame.type !== "hello") {
          ws.close(4008, "hello required");
          return;
        }
        const device = this.authenticateHello(frame.payload);
        if (!device) {
          ws.close(4002, "invalid device credential");
          return;
        }
        authenticated = true;
        clearTimeout(helloTimer);
        this.registerControl(ws, device, frame.payload);
        return;
      }
      this.handleControlFrame(ws, frame);
    });
    ws.on("close", () => {
      clearTimeout(helloTimer);
      this.options.metrics?.controlClosed();
      this.removeControl(ws);
    });
    ws.on("error", () => ws.close());
  }
  private authenticateHello(hello: HelloPayload): DeviceRow | null {
    const device = this.options.store.findDeviceByAccessToken(hello.accessToken);
    if (!device) return null;
    if (device.id !== hello.deviceId || device.role !== hello.role) return null;
    return device;
  }
  private registerControl(ws: WebSocket, device: DeviceRow, hello: HelloPayload): void {
    const existing = this.controlSockets.get(device.id);
    if (existing) existing.ws.close(4003, "connection replaced");
    const capabilities = hello.capabilities.filter((value) => SERVER_CAPABILITIES.includes(value));
    const socket: ControlSocket = { ws, device, capabilities, alive: true };
    this.controlSockets.set(device.id, socket);
    this.options.store.touchDevice(device.id);
    this.sendFrame(ws, {
      v: 1,
      id: randomUUID(),
      timestamp: Date.now(),
      type: "hello.ack",
      payload: {
        protocol: 1,
        connectionSessionId: randomUUID(),
        heartbeatIntervalMs: HEARTBEAT_INTERVAL_MS,
        maxControlFrameBytes: 64 * 1024,
        maxRelayFrameBytes: 1024 * 1024,
        capabilities,
      },
    });
    ws.on("pong", () => {
      socket.alive = true;
    });
    const heartbeat = setInterval(() => {
      if (!socket.alive) {
        ws.close(4001, "heartbeat timeout");
        clearInterval(heartbeat);
        return;
      }
      socket.alive = false;
      ws.ping();
    }, HEARTBEAT_INTERVAL_MS);
    ws.on("close", () => clearInterval(heartbeat));
  }
  private handleControlFrame(ws: WebSocket, frame: ControlFrame): void {
    const device = this.deviceForSocket(ws);
    if (!device) {
      ws.close(4002, "not authenticated");
      return;
    }
    if (frame.type === "ping") {
      this.sendFrame(ws, {
        v: 1,
        id: randomUUID(),
        timestamp: Date.now(),
        type: "pong",
        payload: frame.payload.nonce ? { nonce: frame.payload.nonce } : {},
      });
      return;
    }
    if (frame.type === "connect.request") {
      try {
        const result = this.openConnection(device, frame.payload.hostDeviceId);
        this.sendFrame(ws, {
          v: 1,
          id: randomUUID(),
          timestamp: Date.now(),
          type: "connect.accepted",
          payload: {
            connectionId: result.connectionId,
            host: result.host,
            relay: result.relay,
            ...(result.direct ? { direct: result.direct } : {}),
          },
        });
      } catch (error) {
        this.sendError(ws, error);
      }
      return;
    }
    if (frame.type === "transport.selected" || frame.type === "secure.handshake" || frame.type === "relay") {
      this.routeConnectionFrame(device, frame);
    }
  }
  private routeConnectionFrame(device: DeviceRow, frame: ControlFrame): void {
    if (frame.type !== "transport.selected" && frame.type !== "secure.handshake" && frame.type !== "relay") {
      return;
    }
    const active = this.connections.get(frame.payload.connectionId);
    if (!active) return;
    const peerId = peerDeviceId(active, device.id);
    if (!peerId) return;
    if (frame.type === "transport.selected") {
      active.transport = frame.payload.transport;
    }
    if (
      (frame.type === "secure.handshake" || frame.type === "relay") &&
      frame.payload.targetDeviceId !== peerId
    ) {
      return;
    }
    const peerRelay = active.relaySockets.get(peerId);
    if (frame.type === "relay" && peerRelay && peerRelay.readyState === WebSocket.OPEN) {
      const serialized = JSON.stringify(frame);
      peerRelay.send(serialized);
      this.options.metrics?.relayForwarded(Buffer.byteLength(serialized));
      return;
    }
    const peer = this.controlSockets.get(peerId);
    if (peer && peer.ws.readyState === WebSocket.OPEN) {
      this.sendFrame(peer.ws, frame);
    }
  }
  private removeControl(ws: WebSocket): void {
    for (const [deviceId, socket] of this.controlSockets) {
      if (socket.ws !== ws) continue;
      this.controlSockets.delete(deviceId);
      for (const [connectionId, active] of this.connections) {
        if (active.row.host_device_id === deviceId || active.row.client_device_id === deviceId) {
          this.closeConnection(connectionId, 4001, "peer control disconnected");
        }
      }
      return;
    }
  }
  private deviceForSocket(ws: WebSocket): DeviceRow | null {
    for (const socket of this.controlSockets.values()) {
      if (socket.ws === ws) return socket.device;
    }
    return null;
  }
  private send(deviceId: string, frame: { type: ControlFrame["type"]; payload: unknown }): void {
    const socket = this.controlSockets.get(deviceId);
    if (!socket || socket.ws.readyState !== WebSocket.OPEN) return;
    this.sendFrame(socket.ws, {
      v: 1,
      id: randomUUID(),
      timestamp: Date.now(),
      ...frame,
    } as ControlFrame);
  }

  private sendFrame(ws: WebSocket, frame: ControlFrame): void {
    if (ws.readyState !== WebSocket.OPEN) return;
    ws.send(Buffer.from(encodeControlFrame(frame)).toString("utf8"));
  }

  private sendError(ws: WebSocket, error: unknown): void {
    this.sendFrame(ws, errorFrame(error));
  }
}
