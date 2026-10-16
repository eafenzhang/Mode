/**
 * Host sidecar ↔ 壳 的帧编解码：`[4 字节大端长度][payload]`（形状见 spike 文档 §4.4）。
 *
 * 与 `packages/rpc` 的 `IMessagePassingProtocol` 对齐：载荷就是不透明的 RPC 字节，
 * 两侧都不解释它，帧只在**进程边界**上承担长度分帧。两条硬约束：
 *
 * 1. **按视图边界写**（`byteOffset`/`byteLength`）：`Buffer` 常是共享池上的视图，
 *    直接 `stream.write(view.buffer)` 会把整块池子写出去（spike §4.4 的实测坑）；
 * 2. **收方向要自己校验形状**：`ChannelServer` 假设对端只发合法帧，往它投非 RPC 帧会让
 *    Host 进程直接崩（spike §4.6 的 `TypeError: Cannot read properties of undefined`）。
 *    所以入口在喂给协议层**之前**丢掉形状不合法的帧，并留下可诊断的痕迹。
 */
import type { Writable } from "node:stream";

import { BufferReader, VSBuffer, deserialize } from "@mode/rpc";

/**
 * 单帧上限。既是内存保护（长度前缀是攻击面），也是「读方不会为一个畸形长度分配 GB」的保证；
 * 超限即判定该流不可恢复（长度前缀已经对齐不了），后续字节全部丢弃并只报一次。
 */
export const MAX_FRAME_BYTES = 64 * 1024 * 1024;

/**
 * 合法的「页面 → Host」请求类型。
 *
 * 取值来自 `packages/rpc/src/channels.shared.ts` 的 `RequestType` 声明（协议形状）：
 * `Promise = 100` / `PromiseCancel = 101` / `EventListen = 102` / `EventDispose = 103`。
 * 它是 `const enum`，运行时导入不到（也不由 `@mode/rpc` 入口导出），所以这里按声明写死。
 */
const REQUEST_TYPES = new Set([100, 101, 102, 103]);

/**
 * 帧是否是形状合法的 RPC 请求：**用协议层自己的公开反序列化面解析**，再判定它是不是
 * 一个「首元素是 RequestType」的数组 envelope。
 *
 * 为什么不是简单的「非空 + 首字节 == RequestType」：帧的首字节**不是** RequestType，
 * 而是序列化层的值类型标签——实测一次真客户端请求帧是 `[4, 4, 6, 100, …]`，
 * 解析出来是 `[100, 0, "host", "describe"]`（那个 `4` 是 `Array` 标签）。
 * 所以只有真解析一次才既拦得住垃圾帧、又不会误杀合法帧。
 *
 * 这套判定由 `test/portRoundTrip.test.mts` 两侧夹住：真往返（合法帧必须过）+
 * 畸形帧（必须被丢且 Host 不崩）。协议层换 envelope 形状时，那条用例会先红。
 */
export function isRpcRequestFrame(frame: Uint8Array): boolean {
  let value: unknown;
  try {
    value = deserialize(new BufferReader(VSBuffer.wrap(frame)));
  } catch {
    return false;
  }
  if (!Array.isArray(value) || value.length < 2) return false;
  const type = value[0];
  return typeof type === "number" && REQUEST_TYPES.has(type);
}

/** 按 `byteOffset`/`byteLength` 精确写出视图覆盖的字节。 */
export function writeFrame(stream: Writable, bytes: Uint8Array): void {
  const header = Buffer.alloc(4);
  header.writeUInt32BE(bytes.byteLength, 0);
  stream.write(header);
  stream.write(Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength));
}

/**
 * 返回一个可反复投喂 stdin chunk 的解码器（自管粘包/分片）。
 *
 * `onReject` 在丢帧时被调用一次（文案说明原因），便于在 stderr 上留痕；
 * 长度前缀超限或已判定不可恢复后，解码器进入终止态，不再产出任何帧。
 */
export function createFrameDecoder(
  onFrame: (frame: Uint8Array) => void,
  onReject: (reason: string) => void,
): (chunk: Buffer) => void {
  let pending = Buffer.alloc(0);
  let broken = false;
  return (chunk: Buffer): void => {
    if (broken) return;
    pending = pending.length === 0 ? chunk : Buffer.concat([pending, chunk]);
    for (;;) {
      if (pending.length < 4) return;
      const length = pending.readUInt32BE(0);
      if (length > MAX_FRAME_BYTES) {
        broken = true;
        pending = Buffer.alloc(0);
        onReject(`frame_too_large length=${length} max=${MAX_FRAME_BYTES}（后续字节全部丢弃）`);
        return;
      }
      if (pending.length < 4 + length) return;
      const body = pending.subarray(4, 4 + length);
      pending = pending.subarray(4 + length);
      onFrame(new Uint8Array(body));
    }
  };
}
