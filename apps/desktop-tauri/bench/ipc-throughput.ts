/**
 * IPC 帧吞吐基线（M4a 性能四件之一，口径见路线图「度量与预算」节）。
 *
 * ## 测量面（混用即无效）
 *
 * 测的是 **Host sidecar ↔ 壳 stdio 帧路径的 Node 侧固定开销**：
 * `serialize`（JS 值 → 协议字节）→ `writeFrame`（4 字节大端长度前缀）→
 * `createFrameDecoder`（粘包/分片还原）→ `deserialize`（字节 → JS 值）。
 * 这是流式 `text_delta` 每帧都要付的两段编解码 + 拷贝。
 *
 * **不在本测量面内**（如实声明，不假装覆盖）：
 * - Rust 侧对同一 stdio 字节的读写与内存拷贝（原生成本，Node bench 测不到）；
 * - renderer ↔ 壳的 WebView MessagePort 段（JSCore/WebView2 内部实现）；
 * - base64：**当前帧路径不含 base64**（2026-10-23 路线图基线写的「JSON + base64」
 *   与现役实现不符——stdio 帧是二进制 `@mode/rpc` 序列化 + 长度前缀，无 base64 编解码；
 *   本脚本的实测即为该口径的证据）。
 *
 * ## 统计口径（先于数字写定）
 *
 * - 每种载荷：预热 2 轮 + 正式 5 轮，每轮固定帧数；报**中位数轮**的帧吞吐（MB/s 与帧/s）；
 * - 每帧分配数：`--expose-gc` 下批量前后 `heapUsed` 差值 / 帧数（**估算**，含 GC 抖动，
 *   只用于观察量级与回归对比，不作绝对承诺）；未带 `--expose-gc` 时该项报 N/A；
 * - 载荷三档：control（小控制帧）/ delta（流式文本增量）/ bulk（256 KiB 附件分片）。
 *
 * ```bash
 * node --import tsx --expose-gc apps/desktop-tauri/bench/ipc-throughput.ts
 * ```
 */
import { BufferReader, BufferWriter, VSBuffer, serialize, deserialize } from "@mode/rpc";

import {
  createFrameDecoder,
  writeFrame,
} from "../../../packages/desktop/src/host-standalone/stdioFrames.ts";

/** 每轮帧数（正式轮）；预热轮减半。 */
const FRAMES_PER_ROUND = 2_000;
const WARMUP_ROUNDS = 2;
const MEASURED_ROUNDS = 5;
/** bulk 档载荷大小（与 attachment 分片的实测量级一致，见 LR-4 spike）。 */
const BULK_BYTES = 256 * 1024;

interface Payload {
  name: string;
  frame: () => unknown;
}

const PAYLOADS: Payload[] = [
  // 小控制帧：对应 ChannelClient 的一次请求 envelope（形如 [100, seq, channel, method]）。
  { name: "control", frame: () => [100, 1, "host", "describe"] },
  // 流式文本增量：典型 text_delta 事件（方法名 + 一屏中文文本）。
  {
    name: "delta",
    frame: () => [
      104,
      "task-stream",
      "text_delta",
      { taskId: "task-bench-0001", delta: "增量文本片段，用于模拟流式输出的典型载荷。".repeat(4) },
    ],
  },
  // 大帧：256 KiB 二进制分片（attachment 走 VSBuffer 路径，不经 JSON fallback）。
  {
    name: "bulk",
    frame: () => [104, "attachment", "chunk", VSBuffer.alloc(BULK_BYTES)],
  },
];

/** 可写流替身：`writeFrame` 只用 `write()`；这里收集**真实线上字节**（长度前缀 + 载荷）不真落盘。 */
function collectingWritable(): { write(chunk: Buffer): boolean; take: () => Buffer[]; bytes: () => number } {
  const chunks: Buffer[] = [];
  let total = 0;
  return {
    write(chunk: Buffer) {
      chunks.push(chunk);
      total += chunk.byteLength;
      return true;
    },
    take: () => chunks,
    bytes: () => total,
  };
}

/** 把一个 JS 值按协议序列化成载荷字节（与 `channelClient` 生产路径同源：`BufferWriter` + `serialize`）。 */
function serializeToBytes(value: unknown): VSBuffer {
  const writer = new BufferWriter();
  serialize(writer, value);
  return writer.buffer;
}

interface RoundResult {
  encodeFramesPerSec: number;
  encodeMegabytesPerSec: number;
  roundTripFramesPerSec: number;
  roundTripMegabytesPerSec: number;
  frameBytes: number;
}

/** 一轮完整测量：编码（serialize + writeFrame）+ 解码（decoder + deserialize 往返）。 */
function runRound(payload: Payload, frames: number): RoundResult {
  // 编码：每帧 serialize → writeFrame（长度前缀 + 载荷）写入收集流；
  // 收集到的就是**线上字节**，直接供解码轮使用（不重复拼装，避免测到非生产的路径）。
  const chunks: Buffer[] = [];
  let wireBytes = 0;
  const encodeStart = performance.now();
  for (let index = 0; index < frames; index += 1) {
    const body = serializeToBytes(payload.frame());
    const stream = collectingWritable();
    writeFrame(stream as never, body.buffer);
    wireBytes += stream.bytes();
    chunks.push(...stream.take());
  }
  const encodeMs = performance.now() - encodeStart;

  // 解码：把编码出的全部字节按 1 KiB 碎片投喂解码器（覆盖粘包与分片两种到达形态）。
  const decoded: unknown[] = [];
  const decodeStart = performance.now();
  const decoder = createFrameDecoder(
    (frame) => {
      decoded.push(deserialize(new BufferReader(VSBuffer.wrap(frame))));
    },
    (reason) => {
      throw new Error(`解码拒绝：${reason}`);
    },
  );
  const wire = Buffer.concat(chunks);
  for (let offset = 0; offset < wire.byteLength; offset += 1024) {
    decoder(wire.subarray(offset, Math.min(offset + 1024, wire.byteLength)));
  }
  const decodeMs = performance.now() - decodeStart;
  if (decoded.length !== frames) {
    throw new Error(`解码帧数不符：期望 ${frames}，得到 ${decoded.length}`);
  }

  const megabytes = wireBytes / (1024 * 1024);
  return {
    encodeFramesPerSec: frames / (encodeMs / 1000),
    encodeMegabytesPerSec: megabytes / (encodeMs / 1000),
    roundTripFramesPerSec: frames / ((encodeMs + decodeMs) / 1000),
    roundTripMegabytesPerSec: megabytes / ((encodeMs + decodeMs) / 1000),
    frameBytes: Math.round(wireBytes / frames),
  };
}

/** 批量堆增量估算每帧分配（需 `--expose-gc`；GC 抖动下只看量级）。 */
function heapDeltaPerFrame(payload: Payload, frames: number): number | undefined {
  if (typeof globalThis.gc !== "function") return undefined;
  const gc = globalThis.gc as () => void;
  // 预热：让 JIT 与字符串驻留先稳定，避免首轮把编译成本算进分配。
  runRound(payload, Math.min(frames, 200));
  gc();
  const before = process.memoryUsage().heapUsed;
  runRound(payload, frames);
  const after = process.memoryUsage().heapUsed;
  return Math.max(0, after - before) / frames;
}

function median(values: number[]): number {
  const sorted = [...values].sort((left, right) => left - right);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 0
    ? ((sorted[middle - 1] as number) + (sorted[middle] as number)) / 2
    : (sorted[middle] as number);
}

function main(): void {
  const exposedGc = typeof globalThis.gc === "function";
  console.log(
    `[ipc-throughput] 每轮 ${FRAMES_PER_ROUND} 帧，预热 ${WARMUP_ROUNDS} 轮 + 正式 ${MEASURED_ROUNDS} 轮，` +
      `统计取中位数轮；每帧堆增量：${exposedGc ? "heapUsed 差值（--expose-gc 已启用）" : "N/A（需 --expose-gc）"}`,
  );
  for (const payload of PAYLOADS) {
    for (let round = 0; round < WARMUP_ROUNDS; round += 1) {
      runRound(payload, Math.floor(FRAMES_PER_ROUND / 2));
    }
    const results: RoundResult[] = [];
    for (let round = 0; round < MEASURED_ROUNDS; round += 1) {
      results.push(runRound(payload, FRAMES_PER_ROUND));
    }
    const encodeMbps = median(results.map((result) => result.encodeMegabytesPerSec));
    const encodeFps = median(results.map((result) => result.encodeFramesPerSec));
    const roundTripMbps = median(results.map((result) => result.roundTripMegabytesPerSec));
    const roundTripFps = median(results.map((result) => result.roundTripFramesPerSec));
    const frameBytes = (results[0] as RoundResult).frameBytes;
    const heap = heapDeltaPerFrame(payload, FRAMES_PER_ROUND);
    console.log(
      `[ipc-throughput] ${payload.name}（帧均 ${String(frameBytes)} B）：` +
        `编码 ${encodeMbps.toFixed(1)} MB/s（${encodeFps.toFixed(0)} 帧/s），` +
        `编码+解码往返 ${roundTripMbps.toFixed(1)} MB/s（${roundTripFps.toFixed(0)} 帧/s），` +
        `每帧堆增量 ≈ ${heap === undefined ? "N/A" : `${(heap / 1024).toFixed(1)} KiB`}`,
    );
  }
}

main();
