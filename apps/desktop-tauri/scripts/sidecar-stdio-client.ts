/**
 * 起一个 sidecar 进程并用 `packages/rpc` 的 `ChannelClient` 连上它（stdio 帧）。
 *
 * 抽出来的理由：这段字节管道（`[4 字节大端长度][payload]` 双向）原先只写在
 * `build-host-sidecar.ts` 的自验里，而 M3 Task 5 的「安装后验证」要**再连一次**
 * （这次连的是安装目录里的 sidecar 与随包 Node）。抄第二份就等于让两条验证路径各自漂移，
 * 而它们要证的恰好是同一件事：**产物跑得起来**。
 *
 * 三处刻意的写法（都有失败现场，别顺手改）：
 *
 * 1. **按视图边界写**：`Buffer` 常是共享内存池上的视图，直接写整块 `buffer` 会把池子里
 *    别人的字节一起发出去；
 * 2. **收方向必须包 `Uint8Array`**：`MessagePortProtocol`/帧解码按 `Uint8Array` 判定，
 *    而 Tauri 的 raw 响应是 `ArrayBuffer`（见 spec §3 的硬约束 2）；
 * 3. **收摊顺序**：先 `dispose` 客户端、再关 stdin（Host 的 stdin EOF 是它自己退的依据）、
 *    再等进程真退。Windows 上文件句柄在进程退出前不释放，所以「等退出」不是客套。
 */
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";

import { ChannelClient, Emitter, VSBuffer, type IMessagePassingProtocol } from "@mode/rpc";

export interface SidecarConnection {
  client: ChannelClient;
  pid: number | undefined;
  /** 到目前为止 sidecar 的 stderr 全文（排障用；失败信息几乎总在它里面）。 */
  stderr: () => string;
  /** 等进程退出（有界）：返回退出码，超时返回 null。 */
  waitForExit: (timeoutMs: number) => Promise<number | null>;
  /** 收摊：断客户端 → 关 stdin → kill → 等退出。幂等。 */
  dispose: () => Promise<void>;
}

export interface ConnectSidecarOptions {
  /** 可执行文件（产物自验用 `process.execPath`；安装后验证用安装目录里的 `node.exe`）。 */
  node: string;
  args: string[];
  cwd: string;
  env: NodeJS.ProcessEnv;
  /** 等 `Initialize` 的上限。 */
  readyTimeoutMs: number;
}

export async function connectToSidecar(options: ConnectSidecarOptions): Promise<SidecarConnection> {
  const child: ChildProcessWithoutNullStreams = spawn(options.node, options.args, {
    cwd: options.cwd,
    env: options.env,
    stdio: ["pipe", "pipe", "pipe"],
  });
  let stderr = "";
  child.stderr.on("data", (chunk: Buffer) => {
    stderr += chunk.toString();
  });

  const messages = new Emitter<VSBuffer>();
  let pending = Buffer.alloc(0);
  child.stdout.on("data", (chunk: Buffer) => {
    pending = Buffer.concat([pending, chunk]);
    while (pending.byteLength >= 4) {
      const length = pending.readUInt32BE(0);
      if (pending.byteLength < 4 + length) break;
      messages.fire(VSBuffer.wrap(new Uint8Array(pending.subarray(4, 4 + length))));
      pending = pending.subarray(4 + length);
    }
  });

  const protocol: IMessagePassingProtocol = {
    send: (buffer) => {
      const bytes = buffer.buffer;
      const header = Buffer.alloc(4);
      header.writeUInt32BE(bytes.byteLength, 0);
      child.stdin.write(header);
      child.stdin.write(
        Buffer.from(bytes.buffer as ArrayBuffer, bytes.byteOffset, bytes.byteLength),
      );
    },
    onMessage: messages.event,
  };
  const client = new ChannelClient(protocol);

  await new Promise<void>((resolveInit, rejectInit) => {
    const timer = setTimeout(
      () =>
        rejectInit(new Error(`${options.readyTimeoutMs}ms 内没有 Initialize；stderr:\n${stderr}`)),
      options.readyTimeoutMs,
    );
    client.onDidInitialize(() => {
      clearTimeout(timer);
      resolveInit();
    });
  });

  const exited = new Promise<number | null>((resolveExit) => {
    child.once("exit", (code) => resolveExit(code));
  });
  let disposed = false;
  return {
    client,
    pid: child.pid,
    stderr: () => stderr,
    waitForExit: (timeoutMs) =>
      Promise.race([
        exited,
        new Promise<null>((resolveTimeout) => {
          setTimeout(() => resolveTimeout(null), timeoutMs);
        }),
      ]),
    dispose: async () => {
      if (disposed) return;
      disposed = true;
      client.dispose();
      child.stdin.end();
      child.kill();
      await Promise.race([
        exited,
        new Promise<void>((resolveTimeout) => {
          setTimeout(resolveTimeout, 5_000);
        }),
      ]);
    },
  };
}
