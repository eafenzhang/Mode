/**
 * stdio 长度前缀帧编解码：`[4 字节大端长度][payload]`。
 *
 * 这是 harness 与测试共用的**唯一**实现（宿主替身与测试驱动都走它），避免两边写出不一致的
 * 写帧方式——尤其是 `byteOffset`/`byteLength`：`Buffer` 常是共享池上的视图，
 * 直接 `stream.write(buffer.buffer)` 会把整块池子写出去。
 */
import type { Writable } from "node:stream";

/** 按 `byteOffset`/`byteLength` 精确写出视图覆盖的字节。 */
export function writeFrame(stream: Writable, bytes: Uint8Array): void {
  const header = Buffer.alloc(4);
  header.writeUInt32BE(bytes.byteLength, 0);
  stream.write(header);
  stream.write(Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength));
}

/** 返回一个可反复投喂 stdout/stderr chunk 的解码器（自管粘包/分片）。 */
export function createFrameDecoder(onFrame: (frame: Uint8Array) => void): (chunk: Buffer<ArrayBufferLike>) => void {
  // Node 24 的 `Buffer` 带 `ArrayBufferLike` 泛型：`Buffer.alloc` 是 `Buffer<ArrayBuffer>`，
  // 而 `stream.on("data")` 投进来的是 `Buffer<ArrayBufferLike>`（纯类型标注，无行为变化）。
  let pending: Buffer<ArrayBufferLike> = Buffer.alloc(0);
  return (chunk: Buffer): void => {
    pending = pending.length === 0 ? chunk : Buffer.concat([pending, chunk]);
    for (;;) {
      if (pending.length < 4) return;
      const length = pending.readUInt32BE(0);
      if (pending.length < 4 + length) return;
      const body = pending.subarray(4, 4 + length);
      pending = pending.subarray(4 + length);
      onFrame(new Uint8Array(body));
    }
  };
}
