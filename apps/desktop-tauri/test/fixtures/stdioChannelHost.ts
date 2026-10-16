/**
 * 测试替身：一个最小的「Host」——在 Node 侧跑真实 `packages/rpc` 的 `ChannelServer`，
 * 字节走 stdio 的 `[4 字节大端长度][payload]` 帧。
 *
 * 它来自 M3 Task 0 的端口传输 spike（`docs/plans/2026-10-16-m3-port-transport-spike.md`）：
 * spike 证明「页面里的真 `MessagePort` 可以由一个真实 `ChannelServer` 做对端」，
 * 这份帧格式与 Host 侧一致，因此保留为可回归的验证面。
 *
 * 与真实 Host（Task 1）的差距：没有 `packages/services`、没有 database-startup 流、
 * 用 stdio 帧而不是 `child_process.fork` 的 IPC。不替代 Task 1 的 sidecar。
 */
import { ChannelServer } from "../../../../packages/rpc/src/channelServer.js";
import { VSBuffer } from "../../../../packages/rpc/src/buffer.js";
import { Emitter } from "../../../../packages/rpc/src/foundation.js";
import type { IServerChannel } from "../../../../packages/rpc/src/channels.shared.js";
import type { IMessagePassingProtocol } from "../../../../packages/rpc/src/protocol.js";

const log = (line: string): void => {
  process.stderr.write(`[stdio-channel-host] ${line}\n`);
};

const writeFrame = (bytes: Uint8Array): void => {
  const header = Buffer.alloc(4);
  header.writeUInt32BE(bytes.byteLength, 0);
  process.stdout.write(header);
  process.stdout.write(Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength));
};

const messages = new Emitter<VSBuffer>();
const protocol: IMessagePassingProtocol = {
  send: (buffer) => writeFrame(buffer.buffer),
  onMessage: messages.event,
};

let pending = Buffer.alloc(0);
process.stdin.on("data", (chunk: Buffer) => {
  pending = pending.length === 0 ? chunk : Buffer.concat([pending, chunk]);
  for (;;) {
    if (pending.length < 4) return;
    const length = pending.readUInt32BE(0);
    if (pending.length < 4 + length) return;
    const body = pending.subarray(4, 4 + length);
    pending = pending.subarray(4 + length);
    messages.fire(VSBuffer.wrap(new Uint8Array(body)));
  }
});

const spikeChannel: IServerChannel<string> = {
  async call(_ctx, command, arg) {
    log(`call ${command} ${JSON.stringify(arg)}`);
    if (command === "ping") {
      return { command, echo: arg, hostPid: process.pid, hostVersion: process.version };
    }
    throw new Error(`unknown command ${command}`);
  },
  listen() {
    throw new Error("spike channel has no events");
  },
};

// deferInit = true：先把 channel 注册好，再显式 ready()，避免 Initialize 抢在 registerChannel 前面。
const server = new ChannelServer(protocol, "stdio-channel-host", 1000, true);
server.registerChannel("spike", spikeChannel);
server.ready();
log(`Initialize sent (pid ${process.pid}, node ${process.version})`);
