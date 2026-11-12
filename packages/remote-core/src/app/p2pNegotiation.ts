import type { ControlFrame } from "../domain/controlFrames.js";
import type { IceServerPayload } from "../domain/controlMessages.js";
import { RemoteError } from "../domain/errors.js";
import type { RemoteSocket } from "./controlTypes.js";
import {
  p2pChannelAsSocket,
  waitForChannel,
  type P2PAdapter,
  type P2PChannel,
  type P2PInitiator,
  type P2PResponder,
} from "./p2p.js";

const DEFAULT_P2P_TIMEOUT_MS = 8000;

export interface P2PNegotiationOptions {
  adapter: P2PAdapter;
  timeoutMs?: number;
  send(frame: { type: ControlFrame["type"]; payload: unknown }): void;
  waitForFrame(
    predicate: (frame: ControlFrame) => boolean,
    timeoutMs: number,
  ): Promise<ControlFrame>;
  onError(error: RemoteError): void;
  /** Host 侧通道就绪通知：由 ControlLink 决定是否挂到会话。 */
  onChannelReady?(connectionId: string): void;
}

/**
 * WebRTC P2P 协商状态机（由 ControlLink 持有）。
 *
 * 与链路拆开的原因：协商涉及 initiator/responder 生命周期、候选缓存与通道挂载，
 * 与 Control 帧路由、Noise 运行时无直接关系；独立后各自可单测。
 */
export class P2PNegotiation {
  private readonly options: P2PNegotiationOptions;
  private initiator: P2PInitiator | null = null;
  private initiatorConnectionId: string | null = null;
  private readonly responders = new Map<
    string,
    { channelPromise: Promise<P2PChannel>; responder: P2PResponder }
  >();
  private readonly iceServers = new Map<string, IceServerPayload[]>();
  /** 已建立/协商中的 P2P 对端，按连接登记，链路关闭时统一释放 ICE。 */
  private readonly peers = new Map<string, { close(): void }>();

  constructor(options: P2PNegotiationOptions) {
    this.options = options;
  }

  private get timeoutMs(): number {
    return this.options.timeoutMs ?? DEFAULT_P2P_TIMEOUT_MS;
  }

  setIceServers(connectionId: string, servers: IceServerPayload[]): void {
    this.iceServers.set(connectionId, servers);
  }

  /** Host 收到 `p2p.candidate`：优先交给该连接的 responder，否则交给当前 initiator。 */
  handleCandidate(connectionId: string, candidate: string, sdpMid?: string): void {
    const entry = this.responders.get(connectionId);
    if (entry) {
      entry.responder.addRemoteCandidate(candidate, sdpMid);
      return;
    }
    if (this.initiatorConnectionId === connectionId) {
      this.initiator?.addRemoteCandidate(candidate, sdpMid);
    }
  }

  /** Client 侧：offer → answer → DataChannel；失败/超时返回 undefined 回落 Relay。 */
  negotiateAsInitiator(
    connectionId: string,
    iceServers: IceServerPayload[],
  ): Promise<RemoteSocket | undefined> {
    const initiator = this.options.adapter.createInitiator(iceServers);
    this.initiator = initiator;
    this.initiatorConnectionId = connectionId;
    return new Promise((resolve) => {
      let settled = false;
      const done = (socket?: RemoteSocket) => {
        if (settled) return;
        settled = true;
        this.initiator = null;
        this.initiatorConnectionId = null;
        if (!socket) initiator.close();
        else this.trackPeer(connectionId, initiator, socket);
        resolve(socket);
      };
      const timer = setTimeout(() => done(undefined), this.timeoutMs);
      timer.unref?.();
      initiator.onLocalCandidate((candidate, sdpMid) => {
        this.options.send({
          type: "p2p.candidate",
          payload: {
            connectionId,
            candidate,
            ...(sdpMid !== undefined ? { sdpMid } : {}),
          },
        });
      });
      initiator.onChannel((channel) => {
        clearTimeout(timer);
        done(p2pChannelAsSocket(channel));
      });
      initiator
        .createOffer()
        .then((sdp) => {
          this.options.send({ type: "p2p.offer", payload: { connectionId, sdp } });
          return this.options.waitForFrame(
            (frame) => frame.type === "p2p.answer" && frame.payload.connectionId === connectionId,
            this.timeoutMs,
          );
        })
        .then((answerFrame) => {
          if (settled) return undefined;
          if (answerFrame.type !== "p2p.answer") return undefined;
          return initiator.acceptAnswer(answerFrame.payload.sdp);
        })
        .catch((error: unknown) => {
          this.options.onError(
            error instanceof RemoteError
              ? error
              : new RemoteError("p2p_failed", "P2P 协商失败", { cause: error }),
          );
          done(undefined);
        });
    });
  }

  /** Host 侧：接受对端 offer，回 answer，等 DataChannel 打开后挂到会话。 */
  async acceptOffer(connectionId: string, sdp: string): Promise<void> {
    const responder = this.options.adapter.createResponder(this.iceServers.get(connectionId) ?? []);
    const channelPromise = new Promise<P2PChannel>((resolve) => {
      responder.onChannel(resolve);
    });
    this.responders.set(connectionId, { channelPromise, responder });
    responder.onLocalCandidate((candidate, sdpMid) => {
      this.options.send({
        type: "p2p.candidate",
        payload: {
          connectionId,
          candidate,
          ...(sdpMid !== undefined ? { sdpMid } : {}),
        },
      });
    });
    try {
      await responder.applyOffer(sdp);
      const answer = await responder.createAnswer();
      this.options.send({ type: "p2p.answer", payload: { connectionId, sdp: answer } });
      const channel = await waitForChannel(channelPromise, this.timeoutMs);
      if (channel) {
        this.options.onChannelReady?.(connectionId);
      } else {
        // 未在时限内打开通道：释放对端并让本连接回落 Relay。
        this.responders.delete(connectionId);
        responder.close();
      }
    } catch (error) {
      this.responders.delete(connectionId);
      responder.close();
      this.options.onError(
        error instanceof RemoteError
          ? error
          : new RemoteError("p2p_failed", "P2P 应答失败", { cause: error }),
      );
    }
  }

  /** 通道打开后挂载数据面；`canAttach` 用于跳过已选路或会话已销毁的情况。 */
  attachWhenReady(
    connectionId: string,
    canAttach: () => boolean,
    attach: (socket: RemoteSocket) => void,
  ): void {
    const entry = this.responders.get(connectionId);
    if (!entry || !canAttach()) return;
    void entry.channelPromise.then((channel) => {
      if (!canAttach()) return;
      this.attachChannel(connectionId, entry.responder, channel, attach);
    });
  }

  /** 已确定走 p2p 时等待通道并挂载；没有 responder 视为数据面不可用。 */
  async attachTransport(
    connectionId: string,
    canAttach: () => boolean,
    attach: (socket: RemoteSocket) => void,
  ): Promise<void> {
    const entry = this.responders.get(connectionId);
    if (!entry) throw new RemoteError("p2p_required", "连接没有 p2p 通道");
    const channel = await entry.channelPromise;
    if (!canAttach()) return;
    this.attachChannel(connectionId, entry.responder, channel, attach);
  }

  close(): void {
    this.initiator?.close();
    this.initiator = null;
    this.initiatorConnectionId = null;
    for (const peer of this.peers.values()) peer.close();
    this.peers.clear();
    for (const entry of this.responders.values()) entry.responder.close();
    this.responders.clear();
    this.iceServers.clear();
  }

  private attachChannel(
    connectionId: string,
    responder: P2PResponder,
    channel: P2PChannel,
    attach: (socket: RemoteSocket) => void,
  ): void {
    const socket = p2pChannelAsSocket(channel);
    this.trackPeer(connectionId, responder, socket);
    attach(socket);
  }

  /** 登记 P2P 对端并绑定通道生命周期，避免关闭后残留 ICE 套接字。 */
  private trackPeer(connectionId: string, peer: { close(): void }, socket: RemoteSocket): void {
    this.peers.set(connectionId, peer);
    socket.onClose(() => {
      try {
        peer.close();
      } catch {
        // 已关闭的对端重复 close 允许失败。
      }
      if (this.peers.get(connectionId) === peer) this.peers.delete(connectionId);
    });
  }
}
