import type { BusinessFrame } from "../domain/businessFrames.js";
import { resolveRelayEndpoint } from "../domain/relayEndpoint.js";
import { MAX_SECURE_MESSAGE_BYTES } from "../domain/limits.js";
import { decodeBase64Url, encodeBase64Url } from "./base64url.js";
import type { WebIdentity } from "./identity.js";
import { WebNoiseHandshake, WebNoiseTransport } from "./noise.js";
import { buildWebPrologue } from "./prologue.js";
import { BusinessStreams, type WebBusinessStream } from "./streams.js";
import {
  openBrowserSocket,
  parseControlEnvelope,
  randomId,
  requireString,
  toControlUrl,
  withTimeout,
  type ControlEnvelope,
  type WebSocketLike,
} from "./transport.js";


export interface WebRemoteConnection {
  send(frame: BusinessFrame): void;
  onFrame(callback: (frame: BusinessFrame) => void): () => void;
  onClose(callback: () => void): () => void;
  close(): void;
}

export interface WebControlLinkOptions {
  serverUrl: string;
  deviceId: string;
  accessToken: string;
  identity: WebIdentity;
  connectTimeoutMs?: number;
  /** 断线重连的首次退避；测试可调小。 */
  reconnectBaseMs?: number;
  /** 测试注入点；缺省使用 globalThis.WebSocket。 */
  createSocket?: (url: string) => Promise<WebSocketLike>;
}

/** 浏览器 v3 Client：Control WS + Relay WSS + Noise，带自动重连。 */
export class WebControlLink {
  private readonly options: WebControlLinkOptions;
  private control: WebSocketLike | null = null;
  private relay: WebSocketLike | null = null;
  private transport: WebNoiseTransport | null = null;
  private frameListeners = new Set<(frame: BusinessFrame) => void>();
  private readonly streams = new BusinessStreams((frame) => this.sendBusiness(frame));
  private closeListeners = new Set<() => void>();
  private accepted: { resolve: (payload: Record<string, unknown>) => void; reject: (error: Error) => void } | null =
    null;
  private helloAck: { resolve: () => void; reject: (error: Error) => void } | null = null;
  private step2: { resolve: (data: Uint8Array) => void; reject: (error: Error) => void } | null = null;
  private relayReady: { resolve: () => void; reject: (error: Error) => void } | null = null;
  private heartbeat: ReturnType<typeof setInterval> | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private attempts = 0;
  private shuttingDown = false;
  private closed = false;
  private activeHostId = "";
  private currentConnectionId = "";
  private relayCounter = 0;

  constructor(options: WebControlLinkOptions) {
    this.options = options;
  }

  async start(): Promise<void> {
    await this.openControl();
  }

  async connect(hostDeviceId: string): Promise<WebRemoteConnection> {
    this.activeHostId = hostDeviceId;
    try {
      const connection = await this.establish(hostDeviceId);
      this.attempts = 0;
      return connection;
    } catch (error) {
      this.shutdown();
      throw error;
    }
  }

  close(): void {
    this.shuttingDown = true;
    this.shutdown();
  }

  request(method: string, params: Record<string, unknown> = {}, timeoutMs = 15_000): Promise<unknown> {
    return this.streams.request(method, params, timeoutMs);
  }

  openStream(service: string, metadata: Record<string, unknown> = {}): WebBusinessStream {
    return this.streams.openStream(service, metadata);
  }
  private async openControl(): Promise<void> {
    const control = await this.openSocket(toControlUrl(this.options.serverUrl));
    if (this.shuttingDown) {
      control.close();
      return;
    }
    this.control = control;
    control.addEventListener("message", (event) => {
      if (typeof event.data === "string") this.handleControl(event.data);
    });
    control.addEventListener("close", () => this.handleLoss("control"));
    const ack = new Promise<void>((resolve, reject) => {
      this.helloAck = { resolve, reject };
    });
    this.sendControl("hello", {
      role: "client",
      deviceId: this.options.deviceId,
      accessToken: this.options.accessToken,
      protocols: [1],
      capabilities: ["secure.noise-ik.v1", "transport.relay"],
    });
    await withTimeout(ack, this.options.connectTimeoutMs ?? 5_000, "等待 hello.ack 超时");
    this.startHeartbeat();
  }
  private async establish(hostDeviceId: string): Promise<WebRemoteConnection> {
    if (!this.control) throw new Error("Control 链路未连接");
    this.hostDeviceId = hostDeviceId;
    this.currentConnectionId = "";
    this.relayCounter = 0;
    const accepted = new Promise<Record<string, unknown>>((resolve, reject) => {
      this.accepted = { resolve, reject };
    });
    const preferred = { hostDeviceId, preferredTransports: ["relay"] };
    this.sendControl("connect.request", preferred);
    const payload = await withTimeout(
      accepted,
      this.options.connectTimeoutMs ?? 15_000,
      "等待 connect.accepted 超时",
    );
    const connectionId = requireString(payload.connectionId, "connectionId");
    this.currentConnectionId = connectionId;
    const host = payload.host as { identityKey?: unknown } | undefined;
    const hostIdentityKey = requireString(host?.identityKey, "host.identityKey");
    const relay = payload.relay as { endpoint?: unknown; ticket?: unknown } | undefined;
    const rawEndpoint = requireString(relay?.endpoint, "relay.endpoint");
    const endpoint = resolveRelayEndpoint(rawEndpoint, this.options.serverUrl);
    const ticket = requireString(relay?.ticket, "relay.ticket");

    const relaySocket = await this.openSocket(endpoint);
    if (this.shuttingDown) {
      relaySocket.close();
      throw new Error("链路已关闭");
    }
    this.relay = relaySocket;
    relaySocket.addEventListener("message", (event) => {
      if (typeof event.data === "string") this.handleRelay(event.data);
    });
    relaySocket.addEventListener("close", () => this.handleLoss("relay"));

    const handshake = new WebNoiseHandshake({
      initiator: true,
      localIdentity: this.options.identity,
      responderStaticKey: hostIdentityKey,
      prologue: buildWebPrologue({
        connectionId,
        hostDeviceId,
        clientDeviceId: this.options.deviceId,
      }),
    });
    const step2Promise = new Promise<Uint8Array>((resolve, reject) => {
      this.step2 = { resolve, reject };
    });
    this.sendControl("transport.selected", { connectionId, transport: "relay" });
    this.sendSecureHandshake(connectionId, 1, handshake.writeMessage());
    const step2 = await withTimeout(
      step2Promise,
      this.options.connectTimeoutMs ?? 10_000,
      "等待 Noise 握手超时",
    );
    handshake.readMessage(step2);
    this.transport = new WebNoiseTransport(handshake.split());

    const ready = new Promise<void>((resolve, reject) => {
      this.relayReady = { resolve, reject };
    });
    this.sendOn(relaySocket, "relay.attach", {
      connectionId,
      deviceId: this.options.deviceId,
      peerDeviceId: hostDeviceId,
      ticket,
    });
    await withTimeout(ready, this.options.connectTimeoutMs ?? 10_000, "等待 relay.ready 超时");
    return {
      send: (frame) => this.sendBusiness(frame),
      onFrame: (callback) => {
        this.frameListeners.add(callback);
        return () => this.frameListeners.delete(callback);
      },
      onClose: (callback) => {
        this.closeListeners.add(callback);
        return () => this.closeListeners.delete(callback);
      },
      close: () => {
        this.shuttingDown = true;
        this.shutdown();
      },
    };
  }
  private handleLoss(source: "control" | "relay"): void {
    if (this.shuttingDown || this.closed) return;
    this.clearHeartbeat();
    this.transport = null;
    this.streams.closeAll(new Error("远程链路断开，正在重连"));
    const relay = this.relay;
    this.relay = null;
    relay?.close(1000, `${source} lost`);
    const control = this.control;
    this.control = null;
    control?.close(1000, `${source} lost`);
    this.rejectWaiters(new Error("远程链路断开，正在重连"));
    if (!this.activeHostId) {
      this.shutdown();
      return;
    }
    this.scheduleReconnect();
  }

  private scheduleReconnect(): void {
    if (this.shuttingDown || this.reconnectTimer) return;
    this.attempts += 1;
    if (this.attempts > 10) {
      this.shutdown();
      return;
    }
    const base = Math.min(
      (this.options.reconnectBaseMs ?? 500) * 2 ** (this.attempts - 1),
      5_000,
    );
    const delay = Math.max(300, Math.round(base * (0.5 + Math.random() * 0.5)));
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      void this.reconnect();
    }, delay);
  }

  private async reconnect(): Promise<void> {
    if (this.shuttingDown || this.closed) return;
    try {
      await this.openControl();
      await this.establish(this.activeHostId);
      this.attempts = 0;
    } catch {
      this.scheduleReconnect();
    }
  }

  private handleControl(text: string): void {
    const frame = parseControlEnvelope(text);
    if (!frame) return;
    switch (frame.type) {
      case "hello.ack":
        this.helloAck?.resolve();
        this.helloAck = null;
        return;
      case "connect.accepted":
        this.accepted?.resolve(frame.payload);
        this.accepted = null;
        return;
      case "secure.handshake": {
        if (frame.payload.step !== 2 || typeof frame.payload.data !== "string") return;
        this.step2?.resolve(decodeBase64Url(frame.payload.data, "secure.handshake.data"));
        this.step2 = null;
        return;
      }
      case "pong":
        return;
      case "error":
        this.rejectWaiters(new Error(String(frame.payload.message ?? frame.payload.code ?? "远程错误")));
        return;
      default:
        return;
    }
  }

  private handleRelay(text: string): void {
    const frame = parseControlEnvelope(text);
    if (!frame) return;
    switch (frame.type) {
      case "relay.ready":
        this.relayReady?.resolve();
        this.relayReady = null;
        return;
      case "relay": {
        if (!this.transport || typeof frame.payload.ciphertext !== "string") return;
        try {
          const plaintext = this.transport.receive(
            decodeBase64Url(frame.payload.ciphertext, "relay.ciphertext"),
          );
          const business = JSON.parse(new TextDecoder().decode(plaintext)) as BusinessFrame;
          if (this.streams.handleFrame(business)) return;
          for (const listener of this.frameListeners) listener(business);
        } catch {
          this.rejectWaiters(new Error("Noise transport 认证失败"));
        }
        return;
      }
      case "relay.detach":
        this.handleLoss("relay");
        return;
      default:
        return;
    }
  }

  private sendBusiness(frame: BusinessFrame): void {
    if (!this.transport || !this.relay) throw new Error("Noise 通道未就绪");
    const bytes = new TextEncoder().encode(JSON.stringify(frame));
    if (bytes.length > MAX_SECURE_MESSAGE_BYTES) throw new Error("业务消息超过上限");
    this.sendOn(this.relay, "relay", {
      connectionId: this.currentConnectionId,
      targetDeviceId: this.hostDeviceId,
      counter: this.relayCounter++,
      ciphertext: encodeBase64Url(this.transport.send(bytes)),
    });
  }

  private sendSecureHandshake(connectionId: string, step: 1 | 2, data: Uint8Array): void {
    this.sendControl("secure.handshake", {
      connectionId,
      targetDeviceId: this.hostDeviceId,
      step,
      data: encodeBase64Url(data),
    });
  }

  private sendControl(type: string, payload: Record<string, unknown>): void {
    if (!this.control) throw new Error("Control 链路未连接");
    this.sendOn(this.control, type, payload);
  }

  private sendOn(socket: WebSocketLike, type: string, payload: Record<string, unknown>): void {
    const frame: ControlEnvelope = {
      v: 1,
      id: randomId(),
      timestamp: Date.now(),
      type,
      payload,
    };
    socket.send(JSON.stringify(frame));
  }

  private async openSocket(url: string): Promise<WebSocketLike> {
    if (this.options.createSocket) return await this.options.createSocket(url);
    return await openBrowserSocket(url);
  }

  private startHeartbeat(): void {
    this.clearHeartbeat();
    this.heartbeat = setInterval(() => {
      if (this.closed || !this.control) return;
      try {
        this.sendControl("ping", {});
      } catch {
        this.handleLoss("control");
      }
    }, 25_000);
  }

  private clearHeartbeat(): void {
    if (this.heartbeat) clearInterval(this.heartbeat);
    this.heartbeat = null;
  }

  private rejectWaiters(error: Error): void {
    this.helloAck?.reject(error);
    this.accepted?.reject(error);
    this.step2?.reject(error);
    this.relayReady?.reject(error);
    this.helloAck = null;
    this.accepted = null;
    this.step2 = null;
    this.relayReady = null;
  }

  private shutdown(): void {
    if (this.closed) return;
    this.closed = true;
    this.shuttingDown = true;
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    this.reconnectTimer = null;
    this.clearHeartbeat();
    this.rejectWaiters(new Error("链路已关闭"));
    this.streams.closeAll(new Error("链路已关闭"));
    this.transport = null;
    this.control?.close(1000, "client closed");
    this.control = null;
    this.relay?.close(1000, "client closed");
    this.relay = null;
    for (const listener of this.closeListeners) listener();
    this.closeListeners.clear();
  }

  private hostDeviceId = "";
}

