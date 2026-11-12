import { startHeartbeatLoop } from "./heartbeat.js";
import {
  parseControlEnvelope,
  randomId,
  withTimeout,
  type ControlEnvelope,
  type WebSocketLike,
} from "./transport.js";

export interface WebControlChannelOptions {
  url: string;
  openSocket: (url: string) => Promise<WebSocketLike>;
  hello: Record<string, unknown>;
  timeoutMs: number;
  /** hello.ack 之外的帧交给链路层处理。 */
  onFrame(frame: ControlEnvelope): void;
  onLost(): void;
}

/** 组装并发送一条 Control/Relay envelope。 */
export function sendEnvelope(
  socket: { send(data: string): void },
  type: string,
  payload: Record<string, unknown>,
): void {
  const frame: ControlEnvelope = {
    v: 1,
    id: randomId(),
    timestamp: Date.now(),
    type,
    payload,
  };
  socket.send(JSON.stringify(frame));
}

/**
 * 浏览器 Control WS：hello 握手、心跳、帧解析。
 * 只负责连接生命周期；业务帧与选路状态仍由 WebControlLink 持有。
 */
export class WebControlChannel {
  private readonly options: WebControlChannelOptions;
  private socket: WebSocketLike | null = null;
  private ack: { resolve: () => void } | null = null;
  private stopHeartbeat: (() => void) | null = null;

  constructor(options: WebControlChannelOptions) {
    this.options = options;
  }

  get connected(): boolean {
    return this.socket !== null;
  }

  async open(): Promise<void> {
    const socket = await this.options.openSocket(this.options.url);
    this.socket = socket;
    socket.addEventListener("message", (event) => {
      if (typeof event.data === "string") this.handleMessage(event.data);
    });
    socket.addEventListener("close", () => this.options.onLost());
    const ack = new Promise<void>((resolve) => {
      this.ack = { resolve };
    });
    sendEnvelope(socket, "hello", this.options.hello);
    await withTimeout(ack, this.options.timeoutMs, "等待 hello.ack 超时");
    this.stopHeartbeat = startHeartbeatLoop({
      intervalMs: 25_000,
      sendPing: () => sendEnvelope(socket, "ping", {}),
      onFail: () => this.options.onLost(),
      isClosed: () => this.socket === null,
    });
  }

  send(type: string, payload: Record<string, unknown>): void {
    if (!this.socket) throw new Error("Control 链路未连接");
    sendEnvelope(this.socket, type, payload);
  }

  close(code = 1000, reason = "closed"): void {
    this.stopHeartbeat?.();
    this.stopHeartbeat = null;
    this.ack = null;
    this.socket?.close(code, reason);
    this.socket = null;
  }

  private handleMessage(text: string): void {
    const frame = parseControlEnvelope(text);
    if (!frame) return;
    if (frame.type === "hello.ack") {
      this.ack?.resolve();
      this.ack = null;
      return;
    }
    this.options.onFrame(frame);
  }
}
