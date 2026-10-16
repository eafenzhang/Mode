/**
 * stdout 守卫：**帧流是独占的**，服务日志不许混进去（M3 Task 2 实测出来的第二条危险面）。
 *
 * 硬约束（Task 1 就写在入口文档里）：「stdout **只用于帧**（任何日志都走 stderr），否则字节流
 * 会被日志污染」。Task 1 的 Host 不跑真服务，所以这条只是文档；Task 2 把 `packages/services`
 * 拉进同一进程后它立刻有了后果——**实测**：`createLocalServices` 启动期 `cua-pip-session` 的
 * wiring 诊断经 `console.log` 写 stdout。
 *
 * 帧格式是 `[4 字节大端长度][payload]`，读方按长度前缀分片。混进一行日志的后果不是
 * 「丢一条日志」而是**长度前缀对不齐**：帧边界从此错位，页面的协议层把垃圾帧静默丢弃
 * （症状是「RPC 全挂、页面看不出原因」）。所以这不是洁癖，是协议完整性。
 *
 * 做法：在装载服务**之前**接管 `process.stdout.write`，把非帧写入原样转投 stderr（日志不丢）
 * 并计数留痕。帧写入走守卫安装前捕获的原始写函数，因此**不经过**这层判定。
 *
 * 已知边界（不假装完整）：子进程若继承了本进程的 stdout fd，它的字节会绕过 JS 层直接写进管道，
 * 这层守卫拦不到。真实服务在带子进程时会用管道（本任务的用例覆盖的是进程内写入这条主路径）；
 * 需要拦住子进程继承的那一半，得在 spawn 处收口，属后续任务。
 */
import type { Writable } from "node:stream";

/** 保留的样本条数（诊断用）。 */
const MAX_SAMPLES = 8;

/** 单条样本保留的字符数（只用于定位「谁在写 stdout」）。 */
const SAMPLE_CHARS = 160;

export interface StdoutGuardSnapshot {
  /** 被从 stdout 转走的写入次数。 */
  diverted: number;
  /** 被转走的写入样本（截断）。 */
  samples: string[];
}

interface MutableWritable extends Writable {
  write: (chunk: unknown, encoding?: unknown, callback?: unknown) => boolean;
}

/** 守卫安装前捕获的原始写函数（帧写入走它，不经过非帧判定）。 */
type RawWrite = (chunk: unknown, encoding?: unknown, callback?: unknown) => boolean;

/** 一帧的写入入口 + 守卫状态。 */
export class StdoutGuard {
  #original: RawWrite;
  #target: MutableWritable;
  #inFrame = false;
  #diverted = 0;
  #samples: string[] = [];

  constructor(target: NodeJS.WriteStream = process.stdout) {
    this.#target = target as unknown as MutableWritable;
    this.#original = this.#target.write.bind(this.#target) as RawWrite;
    const patched = (chunk: unknown, encoding?: unknown, callback?: unknown): boolean => {
      if (this.#inFrame) return this.#original(chunk, encoding, callback);
      this.#diverted += 1;
      const text = typeof chunk === "string" ? chunk : String(chunk);
      this.#samples.push(text.slice(0, SAMPLE_CHARS));
      if (this.#samples.length > MAX_SAMPLES) this.#samples.shift();
      // 原样转投 stderr：日志不丢，只是换了通道（多行写入按原样整块转，不做行级拆分）。
      return process.stderr.write(chunk as never, encoding as never, callback as never);
    };
    this.#target.write = patched as MutableWritable["write"];
  }

  /** 唯一的帧写入入口：这一段走原始写函数，不经过守卫判定。 */
  writeFrameBytes(bytes: Uint8Array): void {
    const header = Buffer.alloc(4);
    header.writeUInt32BE(bytes.byteLength, 0);
    const body = Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    this.#inFrame = true;
    try {
      this.#original(header);
      this.#original(body);
    } finally {
      this.#inFrame = false;
    }
  }

  snapshot(): StdoutGuardSnapshot {
    return { diverted: this.#diverted, samples: [...this.#samples] };
  }

  /** 还原（用例与测试用；产品里不需要）。 */
  restore(): void {
    this.#target.write = this.#original as MutableWritable["write"];
  }
}
