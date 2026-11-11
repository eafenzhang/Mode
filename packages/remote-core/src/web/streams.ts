import type { BusinessFrame } from "../domain/businessFrames.js";
import { decodeBase64Url, encodeBase64Url } from "./base64url.js";
import { randomId, withTimeout } from "./transport.js";

export interface WebBusinessStream {
  readonly streamId: number;
  send(bytes: Uint8Array): void;
  close(reason?: string): void;
  onData(callback: (bytes: Uint8Array) => void): () => void;
  onClose(callback: (reason: string) => void): () => void;
}

interface PendingRequest {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
  timer: ReturnType<typeof setTimeout>;
}

class MutableBusinessStream implements WebBusinessStream {
  readonly streamId: number;
  private readonly dataListeners = new Set<(bytes: Uint8Array) => void>();
  private readonly closeListeners = new Set<(reason: string) => void>();
  private closed = false;

  constructor(
    streamId: number,
    private readonly sendFrame: (frame: BusinessFrame) => void,
  ) {
    this.streamId = streamId;
  }

  send(bytes: Uint8Array): void {
    if (this.closed) throw new Error("stream 已关闭");
    this.sendFrame({ v: 1, type: "stream.data", streamId: this.streamId, data: encodeBase64Url(bytes) });
  }

  close(reason = "closed"): void {
    if (this.closed) return;
    this.closed = true;
    this.sendFrame({ v: 1, type: "stream.close", streamId: this.streamId, reason });
    this.deliverClose(reason);
  }

  onData(callback: (bytes: Uint8Array) => void): () => void {
    this.dataListeners.add(callback);
    return () => this.dataListeners.delete(callback);
  }

  onClose(callback: (reason: string) => void): () => void {
    this.closeListeners.add(callback);
    return () => this.closeListeners.delete(callback);
  }

  deliver(data: Uint8Array): void {
    if (this.closed) return;
    for (const listener of this.dataListeners) listener(data);
  }

  deliverClose(reason: string): void {
    if (this.closed && this.closeListeners.size === 0) return;
    this.closed = true;
    for (const listener of this.closeListeners) listener(reason);
    this.dataListeners.clear();
    this.closeListeners.clear();
  }
}

/** 浏览器侧的业务请求与 remote.ws 流复用；生命周期跟随 WebControlLink。 */
export class BusinessStreams {
  private readonly streams = new Map<number, MutableBusinessStream>();
  private readonly pending = new Map<string, PendingRequest>();
  private nextStreamId = 1;

  constructor(private readonly sendFrame: (frame: BusinessFrame) => void) {}

  request(method: string, params: Record<string, unknown> = {}, timeoutMs = 15_000): Promise<unknown> {
    const id = randomId();
    const promise = new Promise<unknown>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`远程 RPC 超时：${method}`));
      }, timeoutMs);
      this.pending.set(id, { resolve, reject, timer });
      this.sendFrame({ v: 1, type: "rpc.request", id, method, params });
    });
    return withTimeout(promise, timeoutMs + 1_000, `远程 RPC 超时：${method}`);
  }

  openStream(service: string, metadata: Record<string, unknown> = {}): WebBusinessStream {
    const streamId = this.nextStreamId++;
    const stream = new MutableBusinessStream(streamId, this.sendFrame);
    this.streams.set(streamId, stream);
    stream.onClose(() => this.streams.delete(streamId));
    this.sendFrame({ v: 1, type: "stream.open", streamId, service, metadata });
    return stream;
  }

  /** 返回 true 表示该业务帧由流/请求层消费，不应再交给高层 frame 监听。 */
  handleFrame(frame: BusinessFrame): boolean {
    switch (frame.type) {
      case "rpc.response":
      case "rpc.error": {
        const pending = this.pending.get(frame.id);
        if (!pending) return false;
        clearTimeout(pending.timer);
        this.pending.delete(frame.id);
        if (frame.type === "rpc.response") pending.resolve(frame.result);
        else pending.reject(new Error(`${frame.code}: ${frame.message}`));
        return true;
      }
      case "stream.data": {
        this.streams.get(frame.streamId)?.deliver(decodeBase64Url(frame.data, "stream.data"));
        return true;
      }
      case "stream.close": {
        const stream = this.streams.get(frame.streamId);
        stream?.deliverClose(frame.reason ?? "remote closed");
        this.streams.delete(frame.streamId);
        return true;
      }
      case "stream.error": {
        const stream = this.streams.get(frame.streamId);
        stream?.deliverClose(`${frame.code}: ${frame.message}`);
        this.streams.delete(frame.streamId);
        return true;
      }
      default:
        return false;
    }
  }

  closeAll(error: Error): void {
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(error);
    }
    this.pending.clear();
    for (const stream of this.streams.values()) stream.deliverClose(error.message);
    this.streams.clear();
  }
}
