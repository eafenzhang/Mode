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
import { applyTransportSelection, markDirectPending } from "./transportSelection.js";
import { dispatchControlFrame } from "./controlFrameHandlers.js";
import { AcceptedWaiters, FrameWaiters } from "./linkWaiters.js";
import { P2PNegotiation } from "./p2pNegotiation.js";
import type { ControlLinkOptions, RemoteConnection, RemoteSocket } from "./controlTypes.js";

// WebRTC/TURN 未实现前不宣告对应 capability；LAN direct 只在 Host 上报地址时实际可用。
const CAPABILITIES = ["secure.noise-ik.v1", "transport.lan", "transport.relay"];

/** Host/Client 共用的 Control 链路：hello、连接授权、Noise 路由、Relay attach。 */
export class ControlLink {
  private readonly options: ControlLinkOptions;
  private readonly runtime: ConnectionRuntime;
  private readonly accepted = new AcceptedWaiters();
  private readonly waiters = new FrameWaiters();
  private p2p: P2PNegotiation | null = null;
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
      serverUrl: options.serverUrl,
      connectSocket: options.connectSocket,
      sendControl: (frame) => this.sendFrame(frame),
      sendOn: (socket, frame) => this.sendOn(socket, frame),
      onError: options.onError,
    });
    this.direct = new DirectAttachments({
      timeoutMs: options.connectTimeoutMs ?? 10_000,
      send: (socket, frame) => this.sendOn(socket, frame),
    });
    if (options.p2p) {
      this.p2p = new P2PNegotiation({
        adapter: options.p2p.adapter,
        ...(options.p2p.timeoutMs !== undefined ? { timeoutMs: options.p2p.timeoutMs } : {}),
        send: (frame) => this.sendFrame(frame),
        waitForFrame: (predicate, timeoutMs) => this.waiters.wait(predicate, timeoutMs),
        onError: (error) => this.options.onError?.(error),
        onChannelReady: (connectionId) => this.maybeAttachP2P(connectionId),
      });
    }
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
    const ackPromise = this.waiters.wait(
      (frame) => frame.type === "hello.ack",
      HELLO_TIMEOUT_MS,
    );
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
    const acceptedPromise = this.accepted.wait(
      hostDeviceId,
      this.options.connectTimeoutMs ?? 15_000,
    );
    const prefersP2P = Boolean(this.options.p2p);
    this.sendFrame({
      type: "connect.request",
      payload: {
        hostDeviceId,
        preferredTransports: prefersP2P ? ["p2p", "lan", "relay"] : ["lan", "relay"],
      },
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
    let p2pSocket: RemoteSocket | undefined;
    if (!directSocket && this.p2p) {
      p2pSocket = await this.p2p.negotiateAsInitiator(connectionId, accepted.iceServers ?? []);
    }
    const session = this.runtime.create("client", peer, connectionId);
    const active = this.runtime.sessions.get(connectionId)!;
    this.sendFrame({
      type: "transport.selected",
      payload: {
        connectionId,
        transport: directSocket ? "lan" : p2pSocket ? "p2p" : "relay",
      },
    });
    this.options.onTransport?.(directSocket ? "lan" : p2pSocket ? "p2p" : "relay");
    session.start();
    if (directSocket) {
      this.runtime.attachDirect(active, directSocket);
    } else if (p2pSocket) {
      this.runtime.attachDirect(active, p2pSocket);
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
    this.p2p?.close();
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
    this.waiters.dispatch(frame);
    dispatchControlFrame(frame, {
      onConnectAccepted: (payload) => this.accepted.resolve(payload.host.deviceId, payload),
      // 只有 Host 接受入站连接；Client 收到该帧说明服务端路由异常，直接忽略。
      onConnectIncoming: (payload) => {
        if (this.options.role === "host") void this.acceptIncoming(payload);
      },
      onTransportSelected: (payload) =>
        this.handleTransportSelected(payload.connectionId, payload.transport),
      onSecureHandshake: (payload) =>
        this.runtime.queueHandshake(
          payload.connectionId,
          payload.step,
          decodeBase64Url(payload.data, "secure.handshake.data"),
        ),
      onRelayDetach: (connectionId) => {
        this.runtime.sessions.get(connectionId)?.session.close("relay detached");
      },
      onError: (payload) => {
        if (payload.connectionId) {
          this.runtime.sessions
            .get(payload.connectionId)
            ?.session.fail(new RemoteError(payload.code, payload.message));
          return;
        }
        this.options.onError?.(new RemoteError(payload.code, payload.message));
      },
      // 只有 Host 接受 WebRTC offer；Client 收到该帧说明路由异常，忽略。
      onP2POffer: (payload) => {
        if (this.options.role === "host") {
          void this.p2p?.acceptOffer(payload.connectionId, payload.sdp);
        }
      },
      onP2PCandidate: (payload) =>
        this.p2p?.handleCandidate(payload.connectionId, payload.candidate, payload.sdpMid),
      onPing: (nonce) => {
        this.sendFrame({ type: "pong", payload: nonce ? { nonce } : {} });
      },
      onPong: () => {
        this.awaitingPong = false;
      },
    });
  }

  private async acceptIncoming(incoming: ConnectIncomingPayload): Promise<void> {
    try {
      if (!incoming.relay) throw new RemoteError("relay_required", "连接缺少 Relay 分配");
      // 直连 socket 可能比 ensurePeer 的 REST 交叉校验更早到达；先登记 ticket。
      if (incoming.direct) {
        this.direct.register(incoming.connectionId, incoming.direct.ticket);
      }
      await this.ensurePeer(incoming.client);
      if (incoming.iceServers) {
        this.p2p?.setIceServers(incoming.connectionId, incoming.iceServers);
      }
      this.runtime.create("host", incoming.client, incoming.connectionId);
      const active = this.runtime.sessions.get(incoming.connectionId)!;
      // runtime.create 默认 Host 已选路（Relay 老路径）；出现直连候选时要等 Client 选择。
      if (incoming.direct) {
        markDirectPending(active, this.options.connectTimeoutMs ?? 10_000);
      }
      const prefersP2P = incoming.p2pPreferred === true;
      active.relay = incoming.relay;
      if (prefersP2P) {
        // P2P：等对端 offer/通道（本端有适配器）或 client 回落 relay 的选路；
        // 不提前 return——连接 ready 后仍要注册业务监听（onConnection）。
        markDirectPending(active, (this.options.p2p?.timeoutMs ?? 8000) + 1500);
        this.maybeAttachP2P(incoming.connectionId);
      } else {
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
    applyTransportSelection({
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
      attachP2P:
        this.p2p && transport === "p2p"
          ? async () => {
              // Host 可能在 p2p.offer 处理时已挂载同一通道；重复 attach 会关闭底层 dc。
              await this.p2p!.attachTransport(
                connectionId,
                () => !active.dataSocket,
                (socket) => this.runtime.attachDirect(active, socket),
              );
            }
          : undefined,
      timeoutMs: this.options.connectTimeoutMs ?? 10_000,
    });
  }

  /** Host：通道就绪且会话尚未选路时，把 P2P 通道挂到数据面。 */
  private maybeAttachP2P(connectionId: string): void {
    const active = this.runtime.sessions.get(connectionId);
    if (!this.p2p || !active) return;
    this.p2p.attachWhenReady(
      connectionId,
      () => this.runtime.sessions.get(connectionId) === active && !active.dataSocket,
      (socket) => this.runtime.attachDirect(active, socket),
    );
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
    this.accepted.rejectAll(error);
    this.waiters.rejectAll(error);
  }

  private clearHeartbeat(): void {
    if (this.heartbeat) clearInterval(this.heartbeat);
    this.heartbeat = null;
  }
}

export { toControlUrl } from "./controlUrl.js";
