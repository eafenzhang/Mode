import { RTCIceCandidate, RTCPeerConnection } from "werift";

import type {
  IceServerPayload,
  P2PAdapter,
  P2PChannel,
  P2PInitiator,
  P2PResponder,
} from "@mode/remote-core";

/**
 * werift（纯 TS WebRTC）实现的 P2P 适配器（阶段 2b）。
 *
 * DataChannel 承载与 LAN direct 相同的 `relay` 信封文本帧；ICE 服务器来自
 * connect.* 下发的 `iceServers`（STUN + coturn 临时凭证）。证书信任按系统 CA。
 */
interface WeriftDataChannel {
  send(data: string | Buffer): void;
  onmessage: ((event: { data: unknown }) => void) | null;
  onopen: (() => void) | null;
  onclose: (() => void) | null;
  close(): void;
  /** SCTP 发送队列水位（werift DataChannel 提供）；媒体面据它丢旧视频帧。 */
  readonly bufferedAmount?: number;
}

function toWeriftIceServers(servers: IceServerPayload[]) {
  return servers.flatMap((server) =>
    server.urls.map((url) => ({
      urls: url,
      ...(server.username && server.credential
        ? { username: server.username, credential: server.credential }
        : {}),
    })),
  );
}

function bindChannel(dc: WeriftDataChannel): P2PChannel {
  const dataListeners = new Set<(text: string) => void>();
  const closeListeners = new Set<(reason: string) => void>();
  let closed = false;
  const emitClose = (): void => {
    if (closed) return;
    closed = true;
    for (const listener of closeListeners) listener("p2p channel closed");
    closeListeners.clear();
  };
  dc.onmessage = (event) => {
    const text = typeof event.data === "string" ? event.data : String(event.data);
    for (const listener of dataListeners) listener(text);
  };
  dc.onclose = () => emitClose();
  return {
    get bufferedAmount() {
      return dc.bufferedAmount ?? 0;
    },
    send: (text) => {
      if (closed) return;
      dc.send(text);
    },
    onData: (callback) => {
      dataListeners.add(callback);
      return () => dataListeners.delete(callback);
    },
    onClose: (callback) => {
      closeListeners.add(callback);
      return () => closeListeners.delete(callback);
    },
    close: () => {
      try {
        dc.close();
      } catch {
        // 已关闭的通道 close 允许失败。
      }
      emitClose();
    },
  };
}

type CandidateCallback = (candidate: string, sdpMid?: string) => void;

abstract class WeriftPeerBase {
  protected readonly pc: RTCPeerConnection;
  private readonly localCandidateCallbacks = new Set<CandidateCallback>();

  constructor(iceServers: IceServerPayload[]) {
    this.pc = new RTCPeerConnection({ iceServers: toWeriftIceServers(iceServers) });
    this.pc.onicecandidate = (event) => {
      const candidate = event.candidate;
      if (!candidate || !candidate.candidate) return;
      for (const callback of this.localCandidateCallbacks) {
        callback(candidate.candidate, candidate.sdpMid);
      }
    };
  }

  onLocalCandidate(callback: CandidateCallback): () => void {
    this.localCandidateCallbacks.add(callback);
    return () => this.localCandidateCallbacks.delete(callback);
  }

  addRemoteCandidate(candidate: string, sdpMid?: string): void {
    try {
      void this.pc.addIceCandidate(
        new RTCIceCandidate({ candidate, ...(sdpMid !== undefined ? { sdpMid } : {}) }),
      );
    } catch {
      // 单个候选解析失败不影响其余候选与后续协商。
    }
  }

  close(): void {
    try {
      this.pc.close();
    } catch {
      // 已关闭的对端重复 close 允许失败。
    }
  }
}

class WeriftInitiator extends WeriftPeerBase implements P2PInitiator {
  private readonly dataChannel: WeriftDataChannel;
  private readonly channelListeners = new Set<(channel: P2PChannel) => void>();
  private resolvedChannel: P2PChannel | null = null;

  constructor(iceServers: IceServerPayload[]) {
    super(iceServers);
    const dc = this.pc.createDataChannel("mode-p2p") as unknown as WeriftDataChannel;
    this.dataChannel = dc;
    dc.onopen = () => {
      this.resolvedChannel = bindChannel(dc);
      for (const listener of this.channelListeners) listener(this.resolvedChannel);
      this.channelListeners.clear();
    };
  }

  get channel(): Promise<P2PChannel> {
    if (this.resolvedChannel) return Promise.resolve(this.resolvedChannel);
    return new Promise((resolve) => this.channelListeners.add(resolve));
  }

  async createOffer(): Promise<string> {
    const offer = await this.pc.createOffer();
    await this.pc.setLocalDescription(offer);
    return offer.sdp;
  }

  async acceptAnswer(sdp: string): Promise<void> {
    await this.pc.setRemoteDescription({ type: "answer", sdp });
  }

  onChannel(callback: (channel: P2PChannel) => void): void {
    if (this.resolvedChannel) callback(this.resolvedChannel);
    this.channelListeners.add(callback);
  }
}

class WeriftResponder extends WeriftPeerBase implements P2PResponder {
  private readonly channelListeners = new Set<(channel: P2PChannel) => void>();
  private resolvedChannel: P2PChannel | null = null;

  constructor(iceServers: IceServerPayload[]) {
    super(iceServers);
    this.pc.ondatachannel = (event) => {
      const dc = ((event as unknown as { channel?: WeriftDataChannel }).channel ??
        (event as unknown as WeriftDataChannel)) as WeriftDataChannel;
      this.resolvedChannel = bindChannel(dc);
      for (const listener of this.channelListeners) listener(this.resolvedChannel);
      this.channelListeners.clear();
    };
  }

  get channel(): Promise<P2PChannel> {
    if (this.resolvedChannel) return Promise.resolve(this.resolvedChannel);
    return new Promise((resolve) => this.channelListeners.add(resolve));
  }

  async applyOffer(sdp: string): Promise<void> {
    await this.pc.setRemoteDescription({ type: "offer", sdp });
  }

  async createAnswer(): Promise<string> {
    const answer = await this.pc.createAnswer();
    await this.pc.setLocalDescription(answer);
    return answer.sdp;
  }

  onChannel(callback: (channel: P2PChannel) => void): void {
    if (this.resolvedChannel) callback(this.resolvedChannel);
    this.channelListeners.add(callback);
  }
}

export function createWeriftP2PAdapter(): P2PAdapter {
  return {
    createInitiator: (iceServers) => new WeriftInitiator(iceServers),
    createResponder: (iceServers) => new WeriftResponder(iceServers),
  };
}
