import type { P2PAdapter } from "../app/p2p.js";
import { P2PNegotiation } from "../app/p2pNegotiation.js";
import type { RemoteSocket } from "../app/controlTypes.js";
import type { ControlFrame } from "../domain/controlFrames.js";
import type { IceServerPayload } from "../domain/controlMessages.js";
import { RemoteError } from "../domain/errors.js";
import type { ControlEnvelope } from "./transport.js";

/**
 * 浏览器侧 P2P 会话：复用 `P2PNegotiation`，只把 Control 帧的收发换成浏览器 envelope。
 *
 * 每次连接（含重连）新建一个实例；协商超时或失败返回 undefined，由调用方回落 Relay。
 */
export class WebP2PSession {
  private readonly options: {
    adapter: P2PAdapter;
    timeoutMs?: number;
    send: (type: string, payload: Record<string, unknown>) => void;
    onError: (error: Error) => void;
  };
  private negotiation: P2PNegotiation | null = null;
  private readonly waiters: Array<{
    predicate: (frame: ControlFrame) => boolean;
    resolve: (frame: ControlFrame) => void;
    reject: (error: Error) => void;
    timer: ReturnType<typeof setTimeout>;
  }> = [];

  constructor(options: WebP2PSession["options"]) {
    this.options = options;
  }

  async open(
    connectionId: string,
    iceServers: IceServerPayload[],
  ): Promise<RemoteSocket | undefined> {
    const negotiation = new P2PNegotiation({
      adapter: this.options.adapter,
      ...(this.options.timeoutMs !== undefined ? { timeoutMs: this.options.timeoutMs } : {}),
      send: (frame) => this.options.send(frame.type, frame.payload as Record<string, unknown>),
      waitForFrame: (predicate, timeoutMs) => this.waitForFrame(predicate, timeoutMs),
      onError: (error) => this.options.onError(error),
    });
    this.negotiation = negotiation;
    const socket = await negotiation.negotiateAsInitiator(connectionId, iceServers);
    if (!socket) this.negotiation = null;
    return socket;
  }

  /** Control 帧先经过这里，命中 p2p.answer 等 waiter 后再交给链路层。 */
  dispatch(frame: ControlEnvelope): void {
    if (this.waiters.length === 0) return;
    const typed = frame as unknown as ControlFrame;
    for (let index = this.waiters.length - 1; index >= 0; index -= 1) {
      const waiter = this.waiters[index]!;
      if (!waiter.predicate(typed)) continue;
      this.waiters.splice(index, 1);
      clearTimeout(waiter.timer);
      waiter.resolve(typed);
    }
  }

  handleCandidate(connectionId: string, candidate: string, sdpMid?: string): void {
    this.negotiation?.handleCandidate(connectionId, candidate, sdpMid);
  }

  close(): void {
    this.negotiation?.close();
    this.negotiation = null;
    for (const waiter of this.waiters.splice(0, this.waiters.length)) {
      clearTimeout(waiter.timer);
      waiter.reject(new RemoteError("p2p_closed", "P2P 会话已关闭", { retryable: true }));
    }
  }

  private waitForFrame(
    predicate: (frame: ControlFrame) => boolean,
    timeoutMs: number,
  ): Promise<ControlFrame> {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        const index = this.waiters.findIndex((entry) => entry.timer === timer);
        if (index >= 0) this.waiters.splice(index, 1);
        reject(new RemoteError("p2p_timeout", "等待 p2p.answer 超时", { retryable: true }));
      }, timeoutMs);
      this.waiters.push({ predicate, resolve, reject, timer });
    });
  }
}

/** 创建并协商一次浏览器 P2P 会话；未打通时不返回 socket（调用方回落 Relay）。 */
export async function openWebP2P(params: {
  adapter: P2PAdapter;
  timeoutMs?: number;
  connectionId: string;
  iceServers: IceServerPayload[];
  send: (type: string, payload: Record<string, unknown>) => void;
  onError: (error: Error) => void;
  /** 协商开始前回调：调用方需要先登记 session 才能收到 answer/candidate。 */
  onSession?: (session: WebP2PSession) => void;
}): Promise<{ session: WebP2PSession; socket: RemoteSocket | undefined }> {
  const session = new WebP2PSession({
    adapter: params.adapter,
    ...(params.timeoutMs !== undefined ? { timeoutMs: params.timeoutMs } : {}),
    send: params.send,
    onError: params.onError,
  });
  params.onSession?.(session);
  const socket = await session.open(params.connectionId, params.iceServers);
  return { session, socket };
}

/** connect.accepted 下发的 ICE 服务器：只接受带 urls 的条目，其余忽略。 */
export function readIceServers(value: unknown): IceServerPayload[] {
  if (!Array.isArray(value)) return [];
  return value.filter((entry): entry is IceServerPayload => {
    const urls = (entry as { urls?: unknown } | null)?.urls;
    return Array.isArray(urls) && urls.length > 0 && urls.every((url) => typeof url === "string");
  });
}
