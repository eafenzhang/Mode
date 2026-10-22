/**
 * 数据根演练用的**最小生产 Host 客户端**。
 *
 * 只做 base 频道需要的 stdio 帧往返（`[4 字节大端长度][payload]`），不模拟页面端口、
 * scoped attachment 或启动闸门——那些由 `apps/desktop-tauri/test/fixtures/productionHost.mts`
 * 覆盖。这里刻意不 import 生产 Host 的类型：它是**另一个包**的入口，只按进程边界交互。
 */
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { Writable } from "node:stream";

import { ChannelClient, Emitter, VSBuffer, type IMessagePassingProtocol } from "@mode/rpc";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "../../../..");
/** 生产 Host 入口（Rust 壳 spawn 的就是这一个文件）。 */
const HOST_ENTRY = resolve(repoRoot, "packages/desktop/src/host-standalone/index.ts");

/** 按 `byteOffset`/`byteLength` 精确写出视图覆盖的字节（Buffer 常是共享池上的视图）。 */
function writeFrame(stream: Writable, bytes: Uint8Array): void {
  const header = Buffer.alloc(4);
  header.writeUInt32BE(bytes.byteLength, 0);
  stream.write(header);
  stream.write(Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength));
}

/** 自管粘包/分片的长度前缀帧解码器。 */
function createFrameDecoder(onFrame: (frame: Uint8Array) => void): (chunk: Buffer) => void {
  let pending: Buffer = Buffer.alloc(0);
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

export interface DataRootDrillHost {
  client: ChannelClient;
  stop(): Promise<void>;
  stderr(): string;
}

const delay = (ms: number): Promise<void> =>
  new Promise((resolveDelay) => setTimeout(resolveDelay, ms));

/** RPC 调用一律带超时：挂住必须是用例失败，不能变成整轮测试静默卡死。 */
export async function callWithTimeout<T>(call: Promise<T>, what: string, ms = 10_000): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      call,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error(`${what} 超时 ${ms}ms`)), ms);
      }),
    ]);
  } finally {
    if (timer) clearTimeout(timer);
  }
}

/**
 * 起生产 Host 并等 Initialize。`home` 与 `dataBaseDir` 由调用方给定且必须隔离；
 * 生产默认 base = homedir，所以演练把两者指向同一目录。
 */
export async function startProductionHost(options: {
  home: string;
  dataBaseDir: string;
}): Promise<DataRootDrillHost> {
  const child: ChildProcessWithoutNullStreams = spawn("node", ["--import", "tsx", HOST_ENTRY], {
    cwd: repoRoot,
    env: {
      ...process.env,
      HOME: options.home,
      USERPROFILE: options.home,
      MODE_DATA_BASE_DIR: options.dataBaseDir,
      MODE_HOST_STARTUP_ID: `data-root-drill-${Date.now()}`,
    },
    stdio: ["pipe", "pipe", "pipe"],
  });
  let stderrText = "";
  child.stderr.on("data", (chunk: Buffer) => {
    stderrText += chunk.toString();
  });

  const messages = new Emitter<VSBuffer>();
  const protocol: IMessagePassingProtocol = {
    send: (buffer) => writeFrame(child.stdin, buffer.buffer),
    onMessage: messages.event,
  };
  child.stdout.on(
    "data",
    createFrameDecoder((frame) => messages.fire(VSBuffer.wrap(frame))),
  );

  const client = new ChannelClient(protocol);
  try {
    await new Promise<void>((resolveInit, rejectInit) => {
      const timer = setTimeout(
        () => rejectInit(new Error(`15s 内没有收到 Host 的 Initialize；stderr:\n${stderrText}`)),
        15_000,
      );
      client.onDidInitialize(() => {
        clearTimeout(timer);
        resolveInit();
      });
    });
  } catch (error) {
    client.dispose();
    child.kill();
    throw error;
  }
  // 让 Host 把真实启动序列跑完（真服务面注册在 Initialize 之后）。
  await delay(50);

  return {
    client,
    stderr: () => stderrText,
    async stop() {
      const exited =
        child.exitCode !== null || child.signalCode !== null
          ? Promise.resolve()
          : new Promise<void>((resolveExit) => {
              const timer = setTimeout(resolveExit, 10_000);
              child.once("exit", () => {
                clearTimeout(timer);
                resolveExit();
              });
            });
      client.dispose();
      child.kill();
      await exited;
      // Windows 上进程退出后文件句柄仍可能短暂占用。
      await delay(150);
    },
  };
}
