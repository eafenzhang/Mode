import { randomUUID } from "node:crypto";

import { decodeBase64Url } from "../domain/base64url.js";
import {
  decodeControlFrame,
  encodeControlFrame,
  type ConnectAcceptedPayload,
  type ConnectIncomingPayload,
  type ControlFrame,
} from "../domain/controlFrames.js";
import type { DeviceDescriptor } from "../domain/descriptor.js";
import { RemoteError } from "../domain/errors.js";
import { HELLO_TIMEOUT_MS } from "../domain/limits.js";
import { ConnectionRuntime } from "./connectionRuntime.js";
import { toControlUrl } from "./controlUrl.js";
import { DirectAttachments, tryOpenDirectSocket } from "./directAttachments.js";
import { markDirectPending, selectTransport } from "./transportSelection.js";
import type { ControlLinkOptions, RemoteConnection, RemoteSocket } from "./controlTypes.js";

// WebRTC/TURN 未实现前不宣告对应 capability；LAN direct 只在 Host 上报地址时实际可用。
const CAPABILITIES = ["secure.noise-ik.v1", "transport.lan", "transport.relay"];

/** Host/Client 共用的 Control 链路：hello、连接授权、Noise 路由、Relay attach。 */
export class ControlLink {
  private readonly options: ControlLinkOptions;
  private readonly runtime: ConnectionRuntime;
  private readonly accepted = new Map<
    string,
    { resolve: (value: ConnectAcceptedPayload) => void; reject: (error: Error) => void }
  >();
  private readonly waiters: Array<{
    predicate: (frame: ControlFrame) => boolean;
    resolve: (frame: ControlFrame) => void;
    reject: (error: Error) => void;
    timer: NodeJS.Timeout;
  }> = [];
  private readonly direct: DirectAttachments;
  private readonly pendingTransport = new Map<string, "lan" | "p2p" | "turn" | "relay">();
  private socket: RemoteSocket | null = null;
  private heartbeat: NodeJS.Timeout | null = null;
  private awaitingPong = false;
  private closed = false;

  constructor(options: ControlLinkOptions) {
    this.options = options;
    this.runtime = new ConnectionRuntime({
      identity: options.identity,
      deviceId: options.deviceId,
      connectSocket: options.connectSocket,
      sendControl: (frame) => this.sendFrame(frame),
      sendOn: (socket, frame) => this.sendOn(socket, frame),
      onError: options.onError,
    });
    this.direct = new DirectAttachments({
      timeoutMs: options.connectTimeoutMs ?? 10_000,
      send: (socket, frame) => this.sendOn(socket, frame),
    });
  }

  async start(): Promise<void> {
    const socket = await this.options.connectSocket(toControlUrl(this.options.serverUrl));
    if (this.closed) {
      socket.close(1000, "closed");
      return;
    }
    this.socket = socket;
    socket.onMessage((text) => this.handleMessage(text));
    socket.onClose((code, reason) => {
      if (this.closed) return;
      this.closed = true;
      this.clearHeartbeat();
      this.runtime.closeAll("control closed");
      this.rejectAccepted(new Error("control closed"));
      this.options.onClose?.({ code, reason });
    });
    const ackPromise = this.waitFor((frame) => frame.type === "hello.ack", HELLO_TIMEOUT_MS);
    this.sendFrame({
      type: "hello",
      payload: {
        role: this.options.role,
        deviceId: this.options.deviceId,
        accessToken: this.options.accessToken,
        protocols: [1],
        capabilities: CAPABILITIES,
        clientVersion: this.options.clientVersion,
      },
    });
    let ack: ControlFrame;
    try {
      ack = await ackPromise;
    } catch (error) {
      // 握手失败时旧 socket 不能留在这里：Host 的重连会创建新链路，
      // 不关闭会积累半开连接，也会让服务端目录长期显示旧连接。
      this.close();
      throw error;
    }
    if (ack.type !== "hello.ack") throw new RemoteError("protocol", "缺少 hello.ack");
    this.heartbeat = setInterval(() => {
      if (this.awaitingPong) {
        // 上一轮 ping 没有 pong：TCP 可能已半开（NAT/休眠/断网），
        // 主动关闭让 onClose 走重连，而不是等到下次业务请求才发现。
        this.clearHeartbeat();
        this.socket?.close(4001, "heartbeat timeout");
        return;
      }
      this.awaitingPong = true;
      try {
        this.sendFrame({ type: "ping", payload: {} });
      } catch {
        this.clearHeartbeat();
      }
    }, ack.payload.heartbeatIntervalMs);
  }

  async connect(hostDeviceId: string): Promise<RemoteConnection> {
    if (this.options.role !== "client") {
      throw new RemoteError("role_required", "只有 Client 可以发起连接");
    }
    const acceptedPromise = this.waitAccepted(hostDeviceId);
    this.sendFrame({
      type: "connect.request",
      payload: { hostDeviceId, preferredTransports: ["lan", "relay"] },
    });
    const accepted = await acceptedPromise;
    const peer = accepted.host;
    await this.ensurePeer(peer);
    if (!accepted.relay) throw new RemoteError("relay_required", "连接缺少 Relay 分配");
    const connectionId = accepted.connectionId;
    const directSocket = await tryOpenDirectSocket({
      direct: accepted.direct,
      connectionId,
      ...(this.options.openDirectSocket ? { open: this.options.openDirectSocket } : {}),
      ...(this.options.onError ? { onError: this.options.onError } : {}),
    });
    const session = this.runtime.create("client", peer, connectionId);
    const active = this.runtime.sessions.get(connectionId)!;
    this.sendFrame({
      type: "transport.selected",
      payload: { connectionId, transport: directSocket ? "lan" : "relay" },
    });
    session.start();
    if (directSocket) {
      this.runtime.attachDirect(active, directSocket);
    } else {
      await this.runtime.attachRelay(
        active,
        accepted.relay,
        this.options.connectTimeoutMs ?? 10_000,
      );
    }
    return active.ready;
  }

  /** Host 的 `/ws/v1/direct` 入口校验 ticket 后调用。 */
  acceptDirectSocket(connectionId: string, ticket: string, socket: RemoteSocket): boolean {
    return this.direct.accept(connectionId, ticket, socket);
  }

  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.clearHeartbeat();
    this.direct.closeAll(new Error("link closed"));
    this.pendingTransport.clear();
    this.runtime.closeAll("link closed");
    this.rejectAccepted(new Error("link closed"));
    this.socket?.close(1000, "link closed");
    this.socket = null;
  }

  private handleMessage(text: string): void {
    let frame: ControlFrame;
    try {
      frame = decodeControlFrame(Buffer.from(text, "utf8"));
    } catch {
      return;
    }
    this.dispatchWaiters(frame);
    switch (frame.type) {
      case "hello.ack":
        return;
      case "connect.accepted": {
        const waiter = this.accepted.get(frame.payload.host.deviceId);
        if (waiter) {
          this.accepted.delete(frame.payload.host.deviceId);
          waiter.resolve(frame.payload);
        }
        return;
      }
      case "connect.incoming":
        // 只有 Host 接受入站连接；Client 收到该帧说明服务端路由异常，直接忽略。
        if (this.options.role === "host") void this.acceptIncoming(frame.payload);
        return;
      case "transport.selected":
        this.handleTransportSelected(frame.payload.connectionId, frame.payload.transport);
        return;
      case "secure.handshake":
        this.runtime.queueHandshake(
          frame.payload.connectionId,
          frame.payload.step,
          decodeBase64Url(frame.payload.data, "secure.handshake.data"),
        );
        return;
      case "relay.detach": {
        const active = this.runtime.sessions.get(frame.payload.connectionId);
        active?.session.close("relay detached");
        return;
      }
      case "error":
        if (frame.payload.connectionId) {
          const active = this.runtime.sessions.get(frame.payload.connectionId);
          active?.session.fail(new RemoteError(frame.payload.code, frame.payload.message));
        } else {
          this.options.onError?.(new RemoteError(frame.payload.code, frame.payload.message));
        }
        return;
      case "ping":
        this.sendFrame({
          type: "pong",
          payload: frame.payload.nonce ? { nonce: frame.payload.nonce } : {},
        });
        return;
      case "pong":
        this.awaitingPong = false;
        return;
      default:
        return;
    }
  }

  private async acceptIncoming(incoming: ConnectIncomingPayload): Promise<void> {
    try {
      if (!incoming.relay) throw new RemoteError("relay_required", "连接缺少 Relay 分配");
      // 直连 socket 可能比 ensurePeer 的 REST 交叉校验更早到达；先登记 ticket。
      if (incoming.direct) {
        this.direct.register(incoming.connectionId, incoming.direct.ticket);
      }
      await this.ensurePeer(incoming.client);
      this.runtime.create("host", incoming.client, incoming.connectionId);
      const active = this.runtime.sessions.get(incoming.connectionId)!;
      // runtime.create 默认 Host 已选路（Relay 老路径）；出现直连候选时要等 Client 选择。
      if (incoming.direct) {
        markDirectPending(active, this.options.connectTimeoutMs ?? 10_000);
      }
      active.relay = incoming.relay;
      const earlySelection = this.pendingTransport.get(incoming.connectionId);
      if (earlySelection) {
        this.pendingTransport.delete(incoming.connectionId);
        this.handleTransportSelected(incoming.connectionId, earlySelection);
      } else if (!incoming.direct) {
        active.transportSelected = true;
        await this.runtime.attachRelay(
          active,
          incoming.relay,
          this.options.connectTimeoutMs ?? 10_000,
        );
      }
      const connection = await active.ready;
      this.options.onConnection?.(connection);
    } catch (error) {
      const normalized =
        error instanceof RemoteError
          ? error
          : new RemoteError("incoming_failed", "接受连接失败", { cause: error });
      this.options.onError?.(normalized);
    }
  }

  private handleTransportSelected(
    connectionId: string,
    transport: "lan" | "p2p" | "turn" | "relay",
  ): void {
    if (this.options.role !== "host") return;
    const active = this.runtime.sessions.get(connectionId);
    if (!active) {
      // transport.selected 可能早于 connect.incoming 的异步 peer 校验完成。
      this.pendingTransport.set(connectionId, transport);
      return;
    }
    if (active.transportSelected) return;
    void selectTransport({
      active,
      transport,
      direct: this.direct,
      attachDirect: (session, socket) => this.runtime.attachDirect(session, socket),
      attachRelay: (session, timeoutMs) => {
        if (!session.relay) {
          return Promise.reject(new RemoteError("relay_required", "连接缺少 Relay 分配"));
        }
        return this.runtime.attachRelay(session, session.relay, timeoutMs);
      },
      timeoutMs: this.options.connectTimeoutMs ?? 10_000,
    })
      .then(() => {
        active.transportSelected = true;
      })
      .catch((error: unknown) => {
        const normalized =
          error instanceof RemoteError
            ? error
            : new RemoteError("transport_failed", "数据面建立失败", { cause: error });
        active.session.fail(normalized);
        this.direct.reject(connectionId, normalized);
      });
  }

  private async ensurePeer(peer: DeviceDescriptor): Promise<void> {
    const fetched = await this.options.fetchPeer(peer.deviceId);
    if (
      fetched.deviceId !== peer.deviceId ||
      fetched.role !== peer.role ||
      fetched.identityKey !== peer.identityKey
    ) {
      throw new RemoteError("identity_mismatch", "对端 descriptor 与账号服务不一致");
    }
    if (this.options.verifyPeer && !(await this.options.verifyPeer(peer))) {
      throw new RemoteError("identity_mismatch", "对端身份与本地 pin 不一致");
    }
  }

  private waitAccepted(hostDeviceId: string): Promise<ConnectAcceptedPayload> {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.accepted.delete(hostDeviceId);
        reject(new RemoteError("connect_timeout", "等待 connect.accepted 超时", { retryable: true }));
      }, this.options.connectTimeoutMs ?? 15_000);
      this.accepted.set(hostDeviceId, {
        resolve: (value) => {
          clearTimeout(timer);
          resolve(value);
        },
        reject: (error) => {
          clearTimeout(timer);
          reject(error);
        },
      });
    });
  }

  private waitFor(
    predicate: (frame: ControlFrame) => boolean,
    timeoutMs: number,
  ): Promise<ControlFrame> {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        const index = this.waiters.findIndex((entry) => entry.timer === timer);
        if (index >= 0) this.waiters.splice(index, 1);
        reject(new RemoteError("timeout", "等待 control frame 超时", { retryable: true }));
      }, timeoutMs);
      this.waiters.push({ predicate, resolve, reject, timer });
    });
  }

  private dispatchWaiters(frame: ControlFrame): void {
    for (let index = this.waiters.length - 1; index >= 0; index -= 1) {
      const waiter = this.waiters[index]!;
      if (!waiter.predicate(frame)) continue;
      this.waiters.splice(index, 1);
      clearTimeout(waiter.timer);
      waiter.resolve(frame);
    }
  }

  private sendFrame(frame: { type: ControlFrame["type"]; payload: unknown }): void {
    if (!this.socket) throw new RemoteError("not_connected", "Control 链路未连接");
    this.sendOn(this.socket, frame);
  }

  private sendOn(
    socket: RemoteSocket,
    frame: { type: ControlFrame["type"]; payload: unknown },
  ): void {
    const full = {
      v: 1,
      id: randomUUID(),
      timestamp: Date.now(),
      ...frame,
    } as ControlFrame;
    socket.send(Buffer.from(encodeControlFrame(full)).toString("utf8"));
  }

  private rejectAccepted(error: Error): void {
    for (const waiter of this.accepted.values()) waiter.reject(error);
    this.accepted.clear();
    for (const waiter of this.waiters.splice(0, this.waiters.length)) {
      clearTimeout(waiter.timer);
      waiter.reject(error);
    }
  }

  private clearHeartbeat(): void {
    if (this.heartbeat) clearInterval(this.heartbeat);
    this.heartbeat = null;
  }
}

export { toControlUrl } from "./controlUrl.js";
