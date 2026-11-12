import type { IceServerPayload } from "../domain/controlMessages.js";
import type { RemoteSocket } from "./controlTypes.js";

/**
 * P2P 数据面通道：承载与 LAN direct 完全相同的 `relay` 信封帧（JSON 文本），
 * 因此 runtime 侧复用同一套 attach/解密逻辑。
 */
export interface P2PChannel {
  send(text: string): void;
  onData(callback: (text: string) => void): void;
  onClose(callback: (reason: string) => void): void;
  close(reason?: string): void;
}

export function p2pChannelAsSocket(channel: P2PChannel): RemoteSocket {
  return {
    send: (text) => channel.send(text),
    close: (code, reason) => channel.close(reason ?? `closed ${code ?? 1000}`),
    onMessage: (callback) => channel.onData(callback),
    onClose: (callback) => channel.onClose((reason) => callback(1000, reason ?? "p2p closed")),
  };
}

/** 发起方（Client）：先 createOffer，等对端 answer，等 DataChannel 打开。 */
export interface P2PInitiator {
  createOffer(): Promise<string>;
  acceptAnswer(sdp: string): Promise<void>;
  onLocalCandidate(callback: (candidate: string, sdpMid?: string) => void): void;
  addRemoteCandidate(candidate: string, sdpMid?: string): void;
  onChannel(callback: (channel: P2PChannel) => void): void;
  close(): void;
}

/** 应答方（Host）：applyOffer → createAnswer，等 DataChannel 打开。 */
export interface P2PResponder {
  applyOffer(sdp: string): Promise<void>;
  createAnswer(): Promise<string>;
  onLocalCandidate(callback: (candidate: string, sdpMid?: string) => void): void;
  addRemoteCandidate(candidate: string, sdpMid?: string): void;
  onChannel(callback: (channel: P2PChannel) => void): void;
  close(): void;
}

/** WebRTC 栈适配器：由宿主（Node werift / 浏览器原生）提供，remote-core 不依赖具体实现。 */
export interface P2PAdapter {
  createInitiator(iceServers: IceServerPayload[]): P2PInitiator;
  createResponder(iceServers: IceServerPayload[]): P2PResponder;
}

/** 带 5s 超时等待一个 DataChannel 打开；未打开返回 undefined（调用方回落 Relay）。 */
export function waitForChannel(
  channelPromise: Promise<P2PChannel>,
  timeoutMs: number,
): Promise<P2PChannel | undefined> {
  return new Promise((resolve) => {
    const timer = setTimeout(() => resolve(undefined), timeoutMs);
    timer.unref?.();
    channelPromise.then(
      (channel) => {
        clearTimeout(timer);
        resolve(channel);
      },
      () => {
        clearTimeout(timer);
        resolve(undefined);
      },
    );
  });
}
