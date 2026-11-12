import type { RemoteSocket } from "../app/controlTypes.js";
import type { P2PAdapter } from "../app/p2p.js";
import type { BusinessFrame } from "../domain/businessFrames.js";
import { resolveRelayEndpoint } from "../domain/relayEndpoint.js";
import { decodeBase64Url, encodeBase64Url } from "./base64url.js";
import { decryptBusinessFrame, encryptBusinessFrame } from "./businessFrames.js";
import { sendEnvelope, WebControlChannel } from "./controlChannel.js";
import type { WebIdentity } from "./identity.js";
import { WebNoiseTransport } from "./noise.js";
import { WebLinkWaiters } from "./linkWaiters.js";
import { runInitiatorHandshake } from "./noiseSetup.js";
import { browserP2PSupported, createWebP2PAdapter } from "./p2pBrowser.js";
import { openWebP2P, readIceServers, type WebP2PSession } from "./p2pSession.js";
import { ReconnectScheduler } from "./reconnect.js";
import { BusinessStreams, type WebBusinessStream } from "./streams.js";
import {
  openBrowserSocket,
  parseControlEnvelope,
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
  /** 浏览器 WebRTC：缺省在支持 RTCPeerConnection 时启用；传 false 强制走 Relay。 */
  p2p?: { adapter?: P2PAdapter; timeoutMs?: number } | false;
}

/** 浏览器 v3 Client：Control WS + Relay WSS + Noise，带自动重连。 */
export class WebControlLink {
  private readonly options: WebControlLinkOptions;
  private control: WebControlChannel | null = null;
  private relay: WebSocketLike | null = null;
  private transport: WebNoiseTransport | null = null;
  private frameListeners = new Set<(frame: BusinessFrame) => void>();
  private readonly streams = new BusinessStreams((frame) => this.sendBusiness(frame));
  private closeListeners = new Set<() => void>();
  private readonly waiters = new WebLinkWaiters();
  private readonly reconnector: ReconnectScheduler;
  private readonly p2pAdapter: P2PAdapter | null;
  private p2pSession: WebP2PSession | null = null;
  private dataSocket: RemoteSocket | null = null;

  private shuttingDown = false;
  private closed = false;
  private activeHostId = "";
  private currentConnectionId = "";
  private hostDeviceId = "";
  private relayCounter = 0;

  constructor(options: WebControlLinkOptions) {
    this.options = options;
    // 缺省启用浏览器原生 WebRTC；显式 false 或环境不支持时只走 Relay。
    const p2p = options.p2p;
    this.p2pAdapter =
      p2p === false
        ? null
        : (p2p?.adapter ?? (browserP2PSupported() ? createWebP2PAdapter() : null));
    this.reconnector = new ReconnectScheduler({
      ...(options.reconnectBaseMs !== undefined ? { baseMs: options.reconnectBaseMs } : {}),
      schedule: () => {
        void this.reconnect();
      },
      giveUp: () => this.shutdown(),
    });
  }

  async start(): Promise<void> {
    await this.openControl();
  }

  async connect(hostDeviceId: string): Promise<WebRemoteConnection> {
    this.activeHostId = hostDeviceId;
    try {
      const connection = await this.establish(hostDeviceId);
      this.reconnector.reset();
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

  request(
    method: string,
    params: Record<string, unknown> = {},
    timeoutMs = 15_000,
  ): Promise<unknown> {
    return this.streams.request(method, params, timeoutMs);
  }

  openStream(service: string, metadata: Record<string, unknown> = {}): WebBusinessStream {
    return this.streams.openStream(service, metadata);
  }
  private async openControl(): Promise<void> {
    const control = new WebControlChannel({
      url: toControlUrl(this.options.serverUrl),
      openSocket: (url) => this.openSocket(url),
      hello: {
        role: "client",
        deviceId: this.options.deviceId,
        accessToken: this.options.accessToken,
        protocols: [1],
        capabilities: [
          "secure.noise-ik.v1",
          "transport.relay",
          ...(this.p2pAdapter ? ["transport.p2p"] : []),
        ],
      },
      timeoutMs: this.options.connectTimeoutMs ?? 5_000,
      onFrame: (frame) => this.handleControl(frame),
      onLost: () => this.handleLoss("control"),
    });
    await control.open();
    if (this.shuttingDown) {
      control.close();
      return;
    }
    this.control = control;
  }

  private async establish(hostDeviceId: string): Promise<WebRemoteConnection> {
    if (!this.control) throw new Error("Control 链路未连接");
    this.hostDeviceId = hostDeviceId;
    this.currentConnectionId = "";
    this.relayCounter = 0;
    const accepted = new Promise<Record<string, unknown>>((resolve, reject) => {
      this.waiters.accepted = { resolve, reject };
    });
    const preferred = {
      hostDeviceId,
      preferredTransports: this.p2pAdapter ? ["p2p", "relay"] : ["relay"],
    };
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
    const iceServers = readIceServers(payload.iceServers);
    const timeoutMs = this.options.p2p === false ? undefined : this.options.p2p?.timeoutMs;
    // 每次连接（含重连）重新协商 P2P；未打通时保持 Relay 语义。
    this.p2pSession?.close();
    const p2p =
      this.p2pAdapter && payload.p2pPreferred === true
        ? await openWebP2P({
            adapter: this.p2pAdapter,
            ...(timeoutMs !== undefined ? { timeoutMs } : {}),
            connectionId,
            iceServers,
            send: (type, framePayload) => this.sendControl(type, framePayload),
            onError: (error) => this.waiters.rejectAll(error),
          })
        : null;
    this.p2pSession = p2p?.session ?? null;
    const dataSocket = p2p?.socket;
    this.dataSocket = dataSocket ?? null;
    if (dataSocket) {
      dataSocket.onMessage((text) => this.handleRelay(text));
      dataSocket.onClose(() => this.handleLoss("p2p"));
    }
    const transport: "p2p" | "relay" = dataSocket ? "p2p" : "relay";

    const step2Promise = new Promise<Uint8Array>((resolve, reject) => {
      this.waiters.step2 = { resolve, reject };
    });
    this.sendControl("transport.selected", { connectionId, transport });
    this.transport = await runInitiatorHandshake({
      identity: this.options.identity,
      responderStaticKey: hostIdentityKey,
      connectionId,
      hostDeviceId,
      clientDeviceId: this.options.deviceId,
      timeoutMs: this.options.connectTimeoutMs ?? 10_000,
      sendStep: (step, data) =>
        this.sendControl("secure.handshake", {
          connectionId,
          targetDeviceId: hostDeviceId,
          step,
          data: encodeBase64Url(data),
        }),
      waitStep2: () => step2Promise,
    });

    if (transport === "relay") {
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
      const ready = new Promise<void>((resolve, reject) => {
        this.waiters.relayReady = { resolve, reject };
      });
      sendEnvelope(relaySocket, "relay.attach", {
        connectionId,
        deviceId: this.options.deviceId,
        peerDeviceId: hostDeviceId,
        ticket,
      });
      await withTimeout(ready, this.options.connectTimeoutMs ?? 10_000, "等待 relay.ready 超时");
    }
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
  private handleLoss(source: "control" | "relay" | "p2p"): void {
    if (this.shuttingDown || this.closed) return;
    this.transport = null;
    this.dataSocket?.close(1000, `${source} lost`);
    this.dataSocket = null;
    this.p2pSession?.close();
    this.p2pSession = null;
    this.streams.closeAll(new Error("远程链路断开，正在重连"));
    const relay = this.relay;
    this.relay = null;
    relay?.close(1000, `${source} lost`);
    const control = this.control;
    this.control = null;
    control?.close(1000, `${source} lost`);
    this.waiters.rejectAll(new Error("远程链路断开，正在重连"));
    if (!this.activeHostId) {
      this.shutdown();
      return;
    }
    this.scheduleReconnect();
  }

  private scheduleReconnect(): void {
    if (this.shuttingDown) return;
    this.reconnector.schedule();
  }

  private async reconnect(): Promise<void> {
    if (this.shuttingDown || this.closed) return;
    try {
      await this.openControl();
      await this.establish(this.activeHostId);
      this.reconnector.reset();
    } catch {
      this.scheduleReconnect();
    }
  }

  private handleControl(frame: ControlEnvelope): void {
    if (this.closed) return;
    this.p2pSession?.dispatch(frame);
    switch (frame.type) {
      case "connect.accepted":
        this.waiters.accepted?.resolve(frame.payload);
        this.waiters.accepted = null;
        return;
      case "secure.handshake": {
        if (frame.payload.step !== 2 || typeof frame.payload.data !== "string") return;
        this.waiters.step2?.resolve(decodeBase64Url(frame.payload.data, "secure.handshake.data"));
        this.waiters.step2 = null;
        return;
      }
      case "p2p.candidate": {
        const connectionId = frame.payload.connectionId;
        const candidate = frame.payload.candidate;
        if (typeof connectionId === "string" && typeof candidate === "string") {
          const sdpMid = frame.payload.sdpMid;
          this.p2pSession?.handleCandidate(
            connectionId,
            candidate,
            typeof sdpMid === "string" ? sdpMid : undefined,
          );
        }
        return;
      }
      case "pong":
        return;
      case "error":
        this.waiters.rejectAll(
          new Error(String(frame.payload.message ?? frame.payload.code ?? "远程错误")),
        );
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
        this.waiters.relayReady?.resolve();
        this.waiters.relayReady = null;
        return;
      case "relay": {
        if (!this.transport || typeof frame.payload.ciphertext !== "string") return;
        try {
          const business = decryptBusinessFrame(frame.payload.ciphertext, this.transport);
          if (this.streams.handleFrame(business)) return;
          for (const listener of this.frameListeners) listener(business);
        } catch {
          this.waiters.rejectAll(new Error("Noise transport 认证失败"));
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
    const target = this.dataSocket ?? this.relay;
    if (!this.transport || !target) throw new Error("Noise 通道未就绪");
    sendEnvelope(target, "relay", {
      connectionId: this.currentConnectionId,
      targetDeviceId: this.hostDeviceId,
      counter: this.relayCounter++,
      ciphertext: encryptBusinessFrame(frame, this.transport),
    });
  }

  private sendControl(type: string, payload: Record<string, unknown>): void {
    if (!this.control) throw new Error("Control 链路未连接");
    this.control.send(type, payload);
  }

  private async openSocket(url: string): Promise<WebSocketLike> {
    if (this.options.createSocket) return await this.options.createSocket(url);
    return await openBrowserSocket(url);
  }

  private shutdown(): void {
    if (this.closed) return;
    this.closed = true;
    this.shuttingDown = true;
    this.reconnector.cancel();
    this.waiters.rejectAll(new Error("链路已关闭"));
    this.streams.closeAll(new Error("链路已关闭"));
    this.transport = null;
    this.dataSocket?.close(1000, "client closed");
    this.dataSocket = null;
    this.p2pSession?.close();
    this.p2pSession = null;
    this.control?.close(1000, "client closed");
    this.control = null;
    this.relay?.close(1000, "client closed");
    this.relay = null;
    for (const listener of this.closeListeners) listener();
    this.closeListeners.clear();
  }
}
