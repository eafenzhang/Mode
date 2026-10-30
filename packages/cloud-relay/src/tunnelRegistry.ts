/**
 * 隧道注册表（docs/specs/wss-relay-mvp.md）：tunnelId → 设备 WSS 连接的进程内映射。
 *
 * 从 server.ts 抽出以保持行数上限；只持有连接期内存状态，不落盘。
 */
import { WebSocket } from "ws";

import { toBuffer, type TunnelConnection } from "./internal.js";
import {
  decodeTunnelDataFrame,
  parseTunnelControl,
  RELAY_HEARTBEAT_MS,
  RELAY_MAX_WS_FRAME_BYTES,
  RELAY_PROTOCOL_VERSION,
  type RelayTunnelControl,
} from "./protocol.js";

export class TunnelRegistry {
  private readonly tunnels = new Map<string, TunnelConnection>();

  constructor(
    private readonly log: (line: string) => void,
    private readonly heartbeatMs: number = RELAY_HEARTBEAT_MS,
  ) {}

  get(tunnelId: string): TunnelConnection | undefined {
    return this.tunnels.get(tunnelId);
  }

  ids(): string[] {
    return [...this.tunnels.keys()];
  }

  /** revoke/运维关闭：先关 WSS 再清理，调用方可重复调用。 */
  close(tunnelId: string, code: number, reason: string): void {
    const tunnel = this.tunnels.get(tunnelId);
    if (!tunnel) {
      return;
    }
    tunnel.ws.close(code, reason);
    this.cleanup(tunnel);
  }

  closeAll(code: number, reason: string): void {
    for (const tunnel of this.tunnels.values()) {
      tunnel.ws.close(code, reason);
      this.cleanup(tunnel);
    }
  }

  register(ws: WebSocket, tunnelId: string): void {
    const previous = this.tunnels.get(tunnelId);
    if (previous) {
      previous.ws.close(1000, "replaced");
      this.cleanup(previous);
    }
    const tunnel: TunnelConnection = {
      tunnelId,
      ws,
      nextStreamId: 1,
      pendingHttp: new Map(),
      pendingWs: new Map(),
      activeWs: new Map(),
      heartbeat: null,
      helloTimeout: setTimeout(() => ws.close(1008, "hello timeout"), 10_000),
    };
    this.tunnels.set(tunnelId, tunnel);
    tunnel.helloTimeout?.unref?.();
    tunnel.heartbeat = setInterval(() => {
      if (ws.readyState !== WebSocket.OPEN) {
        return;
      }
      try {
        ws.ping();
        ws.send(JSON.stringify({ type: "ping", ts: Date.now() }));
      } catch {
        ws.terminate();
      }
    }, this.heartbeatMs);
    tunnel.heartbeat.unref?.();
    ws.on("message", (data, isBinary) => {
      if (isBinary) {
        const frame = decodeTunnelDataFrame(toBuffer(data));
        if (!frame || frame.payload.length > RELAY_MAX_WS_FRAME_BYTES) {
          return;
        }
        const controllerWs = tunnel.activeWs.get(frame.streamId);
        if (!controllerWs || controllerWs.readyState !== WebSocket.OPEN) {
          return;
        }
        if (controllerWs.bufferedAmount > RELAY_MAX_WS_FRAME_BYTES * 2) {
          controllerWs.close(1013, "relay backpressure");
          tunnel.activeWs.delete(frame.streamId);
          return;
        }
        controllerWs.send(frame.payload, { binary: frame.isBinary });
        return;
      }
      const control = parseTunnelControl(data.toString());
      if (control) {
        this.handleControl(tunnel, control);
      }
    });
    ws.on("close", () => this.cleanup(tunnel));
    ws.on("error", () => this.cleanup(tunnel));
  }

  private cleanup(tunnel: TunnelConnection): void {
    if (this.tunnels.get(tunnel.tunnelId) !== tunnel) {
      return;
    }
    this.tunnels.delete(tunnel.tunnelId);
    if (tunnel.heartbeat) {
      clearInterval(tunnel.heartbeat);
    }
    if (tunnel.helloTimeout) {
      clearTimeout(tunnel.helloTimeout);
    }
    for (const [streamId, pending] of tunnel.pendingHttp) {
      clearTimeout(pending.timer);
      pending.resolve({ type: "http-error", streamId, message: "relay tunnel closed" });
    }
    tunnel.pendingHttp.clear();
    for (const [streamId, pending] of tunnel.pendingWs) {
      clearTimeout(pending.timer);
      pending.resolve({ type: "ws-error", streamId, message: "relay tunnel closed" });
    }
    tunnel.pendingWs.clear();
    for (const controllerWs of tunnel.activeWs.values()) {
      controllerWs.close(1011, "relay tunnel closed");
    }
    tunnel.activeWs.clear();
    this.log(`[relay] tunnel offline ${tunnel.tunnelId.slice(0, 8)}…`);
  }

  private handleControl(tunnel: TunnelConnection, control: RelayTunnelControl): void {
    switch (control.type) {
      case "hello": {
        if (control.version !== RELAY_PROTOCOL_VERSION || control.tunnelId !== tunnel.tunnelId) {
          tunnel.ws.close(1008, "unsupported protocol");
          return;
        }
        if (tunnel.helloTimeout) {
          clearTimeout(tunnel.helloTimeout);
          tunnel.helloTimeout = null;
        }
        tunnel.ws.send(
          JSON.stringify({
            type: "ready",
            version: RELAY_PROTOCOL_VERSION,
            publicPath: `/d/${tunnel.tunnelId}`,
          }),
        );
        this.log(`[relay] tunnel online ${tunnel.tunnelId.slice(0, 8)}…`);
        return;
      }
      case "http-response":
      case "http-error": {
        const pending = tunnel.pendingHttp.get(control.streamId);
        if (pending) {
          clearTimeout(pending.timer);
          tunnel.pendingHttp.delete(control.streamId);
          pending.resolve(control);
        }
        return;
      }
      case "ws-ready":
      case "ws-error": {
        const pending = tunnel.pendingWs.get(control.streamId);
        if (pending) {
          clearTimeout(pending.timer);
          tunnel.pendingWs.delete(control.streamId);
          pending.resolve(control);
        }
        return;
      }
      case "ws-close": {
        const controllerWs = tunnel.activeWs.get(control.streamId);
        tunnel.activeWs.delete(control.streamId);
        controllerWs?.close(control.code ?? 1000, control.reason ?? "");
        return;
      }
      case "ping":
        tunnel.ws.send(JSON.stringify({ type: "pong", ts: control.ts }));
        return;
      default:
        return;
    }
  }
}
