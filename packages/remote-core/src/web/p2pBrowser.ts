import type { P2PAdapter, P2PChannel, P2PInitiator, P2PResponder } from "../app/p2p.js";
import type { IceServerPayload } from "../domain/controlMessages.js";

/** 浏览器 DataChannel 的最小结构（remote-core 不引入 DOM lib）。 */
interface NativeDataChannelLike {
  send(data: string): void;
  close(): void;
  addEventListener(type: "message", listener: (event: { data: unknown }) => void): void;
  addEventListener(type: "open", listener: () => void): void;
  addEventListener(type: "close", listener: () => void): void;
}

interface NativeCandidateLike {
  candidate: string;
  sdpMid?: string | null;
}

interface NativePeerConnectionLike {
  onicecandidate: ((event: { candidate: NativeCandidateLike | null }) => void) | null;
  ondatachannel: ((event: { channel: NativeDataChannelLike }) => void) | null;
  createDataChannel(label: string): NativeDataChannelLike;
  createOffer(): Promise<{ type: string; sdp?: string }>;
  createAnswer(): Promise<{ type: string; sdp?: string }>;
  setLocalDescription(description: { type: string; sdp: string }): Promise<void>;
  setRemoteDescription(description: { type: string; sdp: string }): Promise<void>;
  addIceCandidate(candidate: { candidate: string; sdpMid?: string }): Promise<void>;
  close(): void;
}

type PeerConnectionCtor = new (config: {
  iceServers: Array<{ urls: string; username?: string; credential?: string }>;
}) => NativePeerConnectionLike;

export function browserP2PSupported(): boolean {
  return typeof (globalThis as { RTCPeerConnection?: unknown }).RTCPeerConnection === "function";
}

function toNativeIceServers(servers: IceServerPayload[]): Array<{
  urls: string;
  username?: string;
  credential?: string;
}> {
  return servers.flatMap((server) =>
    server.urls.map((url) => ({
      urls: url,
      ...(server.username && server.credential
        ? { username: server.username, credential: server.credential }
        : {}),
    })),
  );
}

function decodeText(data: unknown): string {
  if (typeof data === "string") return data;
  if (data instanceof ArrayBuffer) return new TextDecoder().decode(data);
  return "";
}

function bindDataChannel(dc: NativeDataChannelLike): P2PChannel {
  const dataListeners: Array<(text: string) => void> = [];
  const closeListeners: Array<(reason: string) => void> = [];
  let closed = false;
  const emitClose = (reason = "p2p channel closed"): void => {
    if (closed) return;
    closed = true;
    for (const listener of closeListeners) listener(reason);
  };
  dc.addEventListener("message", (event) => {
    const text = decodeText(event.data);
    if (!text) return;
    for (const listener of dataListeners) listener(text);
  });
  dc.addEventListener("close", () => emitClose());
  return {
    send: (text) => {
      if (closed) return;
      dc.send(text);
    },
    onData: (callback) => {
      dataListeners.push(callback);
    },
    onClose: (callback) => {
      closeListeners.push(callback);
    },
    close: (reason) => {
      try {
        dc.close();
      } catch {
        // 已关闭的通道 close 允许失败。
      }
      emitClose(reason ?? "p2p channel closed");
    },
  };
}

type CandidateCallback = (candidate: string, sdpMid?: string) => void;

abstract class BrowserPeerBase {
  protected readonly pc: NativePeerConnectionLike;
  private readonly localCandidateCallbacks: CandidateCallback[] = [];

  constructor(iceServers: IceServerPayload[]) {
    const Ctor = (globalThis as { RTCPeerConnection?: PeerConnectionCtor }).RTCPeerConnection;
    if (!Ctor) throw new Error("当前环境没有 RTCPeerConnection");
    this.pc = new Ctor({ iceServers: toNativeIceServers(iceServers) });
    this.pc.onicecandidate = (event) => {
      const candidate = event.candidate;
      if (!candidate?.candidate) return;
      for (const callback of this.localCandidateCallbacks) {
        callback(candidate.candidate, candidate.sdpMid ?? undefined);
      }
    };
  }

  onLocalCandidate(callback: CandidateCallback): void {
    this.localCandidateCallbacks.push(callback);
  }

  addRemoteCandidate(candidate: string, sdpMid?: string): void {
    void this.pc
      .addIceCandidate({ candidate, ...(sdpMid !== undefined ? { sdpMid } : {}) })
      .catch(() => {
        // 单个候选失败不阻塞其余候选与协商。
      });
  }

  close(): void {
    try {
      this.pc.close();
    } catch {
      // 已关闭的对端重复 close 允许失败。
    }
  }
}

class BrowserInitiator extends BrowserPeerBase implements P2PInitiator {
  private readonly channelListeners: Array<(channel: P2PChannel) => void> = [];
  private resolvedChannel: P2PChannel | null = null;

  constructor(iceServers: IceServerPayload[]) {
    super(iceServers);
    const dc = this.pc.createDataChannel("mode-p2p");
    dc.addEventListener("open", () => {
      this.resolvedChannel = bindDataChannel(dc);
      for (const listener of this.channelListeners) listener(this.resolvedChannel);
      this.channelListeners.length = 0;
    });
  }

  async createOffer(): Promise<string> {
    const offer = await this.pc.createOffer();
    if (!offer.sdp) throw new Error("createOffer 未返回 SDP");
    await this.pc.setLocalDescription({ type: "offer", sdp: offer.sdp });
    return offer.sdp;
  }

  async acceptAnswer(sdp: string): Promise<void> {
    await this.pc.setRemoteDescription({ type: "answer", sdp });
  }

  onChannel(callback: (channel: P2PChannel) => void): void {
    if (this.resolvedChannel) {
      callback(this.resolvedChannel);
      return;
    }
    this.channelListeners.push(callback);
  }
}

class BrowserResponder extends BrowserPeerBase implements P2PResponder {
  private readonly channelListeners: Array<(channel: P2PChannel) => void> = [];
  private resolvedChannel: P2PChannel | null = null;

  constructor(iceServers: IceServerPayload[]) {
    super(iceServers);
    this.pc.ondatachannel = (event) => {
      this.resolvedChannel = bindDataChannel(event.channel);
      for (const listener of this.channelListeners) listener(this.resolvedChannel);
      this.channelListeners.length = 0;
    };
  }

  async applyOffer(sdp: string): Promise<void> {
    await this.pc.setRemoteDescription({ type: "offer", sdp });
  }

  async createAnswer(): Promise<string> {
    const answer = await this.pc.createAnswer();
    if (!answer.sdp) throw new Error("createAnswer 未返回 SDP");
    await this.pc.setLocalDescription({ type: "answer", sdp: answer.sdp });
    return answer.sdp;
  }

  onChannel(callback: (channel: P2PChannel) => void): void {
    if (this.resolvedChannel) {
      callback(this.resolvedChannel);
      return;
    }
    this.channelListeners.push(callback);
  }
}

/** 浏览器原生 `RTCPeerConnection` 适配器：与 Node werift 适配器同契约。 */
export function createWebP2PAdapter(): P2PAdapter {
  return {
    createInitiator: (iceServers) => new BrowserInitiator(iceServers),
    createResponder: (iceServers) => new BrowserResponder(iceServers),
  };
}
