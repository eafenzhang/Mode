import { randomUUID } from "node:crypto";
import { WebSocket } from "ws";

import {
  RELAY_ATTACH_TIMEOUT_MS,
  decodeControlFrame,
  type ControlFrame,
} from "@mode/remote-core";

import { toBytes, type ActiveConnection } from "./gatewayTypes.js";
import { verifyRelayTicket } from "./tickets.js";

export interface RelayChannelContext {
  secret: string;
  connections: Map<string, ActiveConnection>;
  sendFrame(ws: WebSocket, frame: ControlFrame): void;
  closeConnection(id: string, code: number, reason: string): void;
  onRelayForward?: (bytes: number) => void;
}

/** 独立 Relay data WebSocket：只验证签名 ticket 并转发密文。 */
export function handleRelaySocket(ws: WebSocket, context: RelayChannelContext): void {
  let bound: { connectionId: string; deviceId: string; peerDeviceId: string } | null = null;
  const timer = setTimeout(() => {
    if (!bound) ws.close(4008, "relay.attach timeout");
  }, RELAY_ATTACH_TIMEOUT_MS);
  ws.on("message", (data, isBinary) => {
    if (isBinary) {
      ws.close(4008, "binary relay frame");
      return;
    }
    let frame: ControlFrame;
    try {
      frame = decodeControlFrame(toBytes(data));
    } catch {
      ws.close(4008, "invalid relay frame");
      return;
    }
    if (!bound) {
      if (frame.type !== "relay.attach") {
        ws.close(4008, "relay.attach required");
        return;
      }
      const ticket = verifyRelayTicket(context.secret, frame.payload.ticket);
      if (
        !ticket ||
        ticket.connectionId !== frame.payload.connectionId ||
        ticket.deviceId !== frame.payload.deviceId ||
        ticket.peerDeviceId !== frame.payload.peerDeviceId
      ) {
        ws.close(4012, "invalid relay ticket");
        return;
      }
      const active = context.connections.get(frame.payload.connectionId);
      if (!active) {
        ws.close(4008, "unknown connection");
        return;
      }
      bound = {
        connectionId: frame.payload.connectionId,
        deviceId: frame.payload.deviceId,
        peerDeviceId: frame.payload.peerDeviceId,
      };
      clearTimeout(timer);
      const previous = active.relaySockets.get(bound.deviceId);
      if (previous && previous.readyState === WebSocket.OPEN) previous.close(1000, "replaced");
      active.relaySockets.set(bound.deviceId, ws);
      active.relayReady.add(bound.deviceId);
      if (active.relayReady.size === 2) {
        for (const deviceId of active.relayReady) {
          const socket = active.relaySockets.get(deviceId);
          if (socket && socket.readyState === WebSocket.OPEN) {
            context.sendFrame(socket, {
              v: 1,
              id: randomUUID(),
              timestamp: Date.now(),
              type: "relay.ready",
              payload: { connectionId: bound.connectionId },
            });
          }
        }
      }
      return;
    }
    if (frame.type !== "relay" || frame.payload.connectionId !== bound.connectionId) {
      ws.close(4008, "invalid relay frame");
      return;
    }
    if (frame.payload.targetDeviceId !== bound.peerDeviceId) return;
    const active = context.connections.get(bound.connectionId);
    if (!active) return;
    const peer = active.relaySockets.get(bound.peerDeviceId);
    if (peer && peer.readyState === WebSocket.OPEN) {
      const serialized = JSON.stringify(frame);
      peer.send(serialized);
      context.onRelayForward?.(Buffer.byteLength(serialized));
    }
  });
  ws.on("close", () => {
    clearTimeout(timer);
    if (!bound) return;
    const active = context.connections.get(bound.connectionId);
    if (!active) return;
    active.relaySockets.delete(bound.deviceId);
    active.relayReady.delete(bound.deviceId);
    const peer = active.relaySockets.get(bound.peerDeviceId);
    if (peer && peer.readyState === WebSocket.OPEN) {
      context.sendFrame(peer, {
        v: 1,
        id: randomUUID(),
        timestamp: Date.now(),
        type: "relay.detach",
        payload: { connectionId: bound.connectionId, reason: "peer detached" },
      });
    }
    context.closeConnection(bound.connectionId, 4001, "relay detached");
  });
  ws.on("error", () => ws.close());
}
