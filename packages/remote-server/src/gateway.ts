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
} from "@mode/remote-core";

import {
  SERVER_CAPABILITIES,
  errorFrame,
  peerDeviceId,
  rejectDisallowedOrigin,
  toBytes,
  type ActiveConnection,
  type ConnectionResult,
  type ControlSocket,
  type GatewayOptions,
} from "./gatewayTypes.js";
import { handleRelaySocket } from "./relayChannel.js";
import { openConnectionFor, type GatewayConnectionContext } from "./gatewayConnection.js";
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

  private connectionContext(): GatewayConnectionContext {
    return {
      options: this.options,
      connections: this.connections,
      controlSockets: this.controlSockets,
      send: (deviceId, frame) => this.send(deviceId, frame),
      closeConnection: (connectionId, code, reason) =>
        this.closeConnection(connectionId, code, reason),
    };
  }

  /** 由 REST /api/v1/connections 与 Control connect.request 共用。 */
  openConnection(client: DeviceRow, hostDeviceId: string, p2pPreferred = false): ConnectionResult {
    // 只有 Host 在 hello 里宣告了 transport.p2p 才允许协商，避免对端等 8s 超时。
    const hostSupportsP2P =
      this.controlSockets.get(hostDeviceId)?.capabilities.includes("transport.p2p") ?? false;
    return openConnectionFor(
      this.connectionContext(),
      client,
      hostDeviceId,
      p2pPreferred && hostSupportsP2P,
    );
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
        payload: {
          code: "CONNECTION_FAILED",
          message: reason,
          connectionId,
          retryable: code === 4001,
        },
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
        const result = this.openConnection(
          device,
          frame.payload.hostDeviceId,
          frame.payload.preferredTransports.includes("p2p"),
        );
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
            ...(result.p2pPreferred ? { p2pPreferred: true } : {}),
            ...(result.iceServers ? { iceServers: result.iceServers } : {}),
          },
        });
      } catch (error) {
        this.sendError(ws, error);
      }
      return;
    }
    if (
      frame.type === "transport.selected" ||
      frame.type === "secure.handshake" ||
      frame.type === "relay" ||
      frame.type === "p2p.offer" ||
      frame.type === "p2p.answer" ||
      frame.type === "p2p.candidate" ||
      frame.type === "p2p.close"
    ) {
      this.routeConnectionFrame(device, frame);
    }
  }
  private routeConnectionFrame(device: DeviceRow, frame: ControlFrame): void {
    const isP2PSignal =
      frame.type === "p2p.offer" ||
      frame.type === "p2p.answer" ||
      frame.type === "p2p.candidate" ||
      frame.type === "p2p.close";
    if (
      frame.type !== "transport.selected" &&
      frame.type !== "secure.handshake" &&
      frame.type !== "relay" &&
      !isP2PSignal
    ) {
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
    // p2p 信令没有 targetDeviceId：连接的双方就是唯一的对端，connectionId 已绑定身份。
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
