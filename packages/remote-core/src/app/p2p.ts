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

/**
 * DataChannel 单条消息上限：werift/浏览器协商的 SCTP max-message-size 常见为 64KB。
 * 片段还要再套一层 JSON 字符串（引号/反斜杠转义最坏放大 2 倍），因此按 24KB 原文切分。
 */
const CHUNK_BYTES = 24 * 1024;
const CHUNK_PREFIX = '{"__p2pChunk":1,';

/**
 * 业务帧超过单条 DataChannel 上限时按 UTF-8 字节切分，接收端按顺序拼回原文。
 */
function chunkText(text: string): string[] {
  const parts: string[] = [];
  let start = 0;
  while (start < text.length) {
    let bytes = 0;
    let end = start;
    while (end < text.length) {
      const code = text.codePointAt(end)!;
      const width = code > 0xffff ? 2 : 1;
      const cost = code <= 0x7f ? 1 : code <= 0x7ff ? 2 : code <= 0xffff ? 3 : 4;
      if (bytes + cost > CHUNK_BYTES) break;
      bytes += cost;
      end += width;
    }
    // 单个码点就超限（不会发生）：至少前进一次，避免死循环。
    if (end === start) end = Math.min(start + 2, text.length);
    parts.push(text.slice(start, end));
    start = end;
  }
  return parts;
}

export function p2pChannelAsSocket(channel: P2PChannel): RemoteSocket {
  const listeners = new Set<(text: string) => void>();
  let pending: { total: number; parts: string[]; next: number } | null = null;
  const assemble = (text: string): string | undefined => {
    if (!text.startsWith(CHUNK_PREFIX)) return text;
    let frame: { i?: unknown; n?: unknown; d?: unknown };
    try {
      frame = JSON.parse(text) as { i?: unknown; n?: unknown; d?: unknown };
    } catch {
      return text;
    }
    if (typeof frame.i !== "number" || typeof frame.n !== "number" || typeof frame.d !== "string") {
      return text;
    }
    if (!pending || pending.total !== frame.n || frame.i !== pending.next) {
      // 分片缺失或乱序：丢弃半条消息，等待下一条完整消息。
      pending = null;
      if (frame.i !== 0) return undefined;
      pending = { total: frame.n, parts: [], next: 0 };
    }
    pending.parts.push(frame.d);
    pending.next += 1;
    if (pending.next < pending.total) return undefined;
    const full = pending.parts.join("");
    pending = null;
    return full;
  };
  channel.onData((text) => {
    const payload = assemble(text);
    if (payload === undefined) return;
    for (const listener of listeners) listener(payload);
  });
  return {
    send: (text) => {
      const parts = chunkText(text);
      if (parts.length === 1) {
        channel.send(text);
        return;
      }
      parts.forEach((data, index) => {
        channel.send(JSON.stringify({ __p2pChunk: 1, i: index, n: parts.length, d: data }));
      });
    },
    close: (code, reason) => channel.close(reason ?? `closed ${code ?? 1000}`),
    onMessage: (callback) => {
      listeners.add(callback);
    },
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
