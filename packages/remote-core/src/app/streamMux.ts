import { decodeBase64Url, encodeBase64Url } from "../domain/base64url.js";
import type {
  BusinessFrame,
  StreamCloseFrame,
  StreamDataFrame,
  StreamErrorFrame,
  StreamOpenFrame,
} from "../domain/businessFrames.js";
import { RemoteError } from "../domain/errors.js";

export interface RemoteStream {
  readonly streamId: number;
  readonly service: string;
  readonly metadata: Record<string, unknown>;
  send(data: Uint8Array): void;
  close(reason?: string): void;
  onData(callback: (data: Uint8Array) => void): () => void;
  onClose(callback: (reason: string) => void): () => void;
}

export interface StreamMuxOptions {
  send: (frame: BusinessFrame) => void;
  onStream: (stream: RemoteStream) => void;
}

const MAX_CONCURRENT_STREAMS = 64;
const MAX_STREAM_ID = 0x7fffffff;

/** Noise 通道内的有界流多路复用；流状态只属于当前连接。 */
export class StreamMux {
  private readonly options: StreamMuxOptions;
  private readonly streams = new Map<number, MutableStream>();
  private nextStreamId = 1;

  constructor(options: StreamMuxOptions) {
    this.options = options;
  }

  open(service: string, metadata: Record<string, unknown> = {}): RemoteStream {
    if (this.streams.size >= MAX_CONCURRENT_STREAMS) {
      throw new RemoteError("stream_limit", "并发流超过上限");
    }
    if (this.nextStreamId > MAX_STREAM_ID) {
      this.nextStreamId = 1;
    }
    let streamId = this.nextStreamId;
    while (this.streams.has(streamId)) {
      streamId += 1;
      if (streamId > MAX_STREAM_ID) streamId = 1;
    }
    this.nextStreamId = streamId + 1;
    const stream = new MutableStream(streamId, service, metadata, this.options.send);
    this.streams.set(streamId, stream);
    this.options.send({ v: 1, type: "stream.open", streamId, service, metadata });
    return stream;
  }

  handle(frame: BusinessFrame): boolean {
    switch (frame.type) {
      case "stream.open": {
        this.acceptOpen(frame);
        return true;
      }
      case "stream.data": {
        const stream = this.streams.get(frame.streamId);
        if (!stream) throw new RemoteError("unknown_stream", "收到未知流的数据");
        stream.deliver(decodeBase64Url(frame.data, "stream.data"));
        return true;
      }
      case "stream.close": {
        const stream = this.streams.get(frame.streamId);
        if (!stream) return true;
        this.streams.delete(frame.streamId);
        stream.deliverClose(frame.reason ?? "closed");
        return true;
      }
      case "stream.error": {
        const stream = this.streams.get(frame.streamId);
        if (!stream) return true;
        this.streams.delete(frame.streamId);
        stream.deliverError(frame.code, frame.message);
        return true;
      }
      default:
        return false;
    }
  }

  dispose(reason = "disposed"): void {
    for (const [streamId, stream] of this.streams) {
      this.streams.delete(streamId);
      stream.deliverClose(reason);
    }
  }

  private acceptOpen(frame: StreamOpenFrame): void {
    if (this.streams.size >= MAX_CONCURRENT_STREAMS) {
      this.options.send({
        v: 1,
        type: "stream.error",
        streamId: frame.streamId,
        code: "stream_limit",
        message: "并发流超过上限",
      });
      return;
    }
    if (this.streams.has(frame.streamId)) {
      throw new RemoteError("duplicate_stream", "重复的 streamId");
    }
    const stream = new MutableStream(
      frame.streamId,
      frame.service,
      frame.metadata,
      this.options.send,
    );
    this.streams.set(frame.streamId, stream);
    this.options.onStream(stream);
  }
}

class MutableStream implements RemoteStream {
  readonly streamId: number;
  readonly service: string;
  readonly metadata: Record<string, unknown>;
  private readonly sendFrame: (frame: BusinessFrame) => void;
  private readonly dataListeners = new Set<(data: Uint8Array) => void>();
  private readonly closeListeners = new Set<(reason: string) => void>();
  private closed = false;

  constructor(
    streamId: number,
    service: string,
    metadata: Record<string, unknown>,
    sendFrame: (frame: BusinessFrame) => void,
  ) {
    this.streamId = streamId;
    this.service = service;
    this.metadata = metadata;
    this.sendFrame = sendFrame;
  }

  send(data: Uint8Array): void {
    if (this.closed) throw new RemoteError("stream_closed", "流已关闭");
    const frame: StreamDataFrame = {
      v: 1,
      type: "stream.data",
      streamId: this.streamId,
      data: encodeBase64Url(data),
    };
    this.sendFrame(frame);
  }

  close(reason?: string): void {
    if (this.closed) return;
    this.closed = true;
    const frame: StreamCloseFrame = { v: 1, type: "stream.close", streamId: this.streamId };
    if (reason) frame.reason = reason;
    this.sendFrame(frame);
    this.deliverClose(reason ?? "closed");
  }

  onData(callback: (data: Uint8Array) => void): () => void {
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
    this.closed = true;
    for (const listener of this.closeListeners) listener(reason);
    this.dataListeners.clear();
    this.closeListeners.clear();
  }

  deliverError(code: string, message: string): void {
    if (this.closed) return;
    this.closed = true;
    const frame: StreamErrorFrame = {
      v: 1,
      type: "stream.error",
      streamId: this.streamId,
      code,
      message,
    };
    this.sendFrame(frame);
    this.deliverClose(message);
  }
}
