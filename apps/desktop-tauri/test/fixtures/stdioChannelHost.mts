/**
 * 测试替身：一个最小的「Host」——在 Node 侧跑真实 `@mode/rpc` 的 `ChannelServer`，
 * 字节走 stdio 的 `[4 字节大端长度][payload]` 帧（编解码见 `./stdioFrames.ts`）。
 *
 * 它来自 M3 Task 0 的端口传输 harness（`docs/plans/2026-10-16-m3-port-transport-spike.md`）：
 * harness 的壳与测试都 spawn 这同一个文件，保证「被验证的 Host」只有一份。
 *
 * 与真实 Host（Task 1）的差距：没有 `packages/services`、没有 database-startup 流、
 * 用 stdio 帧而不是 `child_process.fork` 的 IPC。不替代 Task 1 的 sidecar。
 */
import { ChannelServer, Emitter, VSBuffer, type IServerChannel, type IMessagePassingProtocol } from "@mode/rpc";

import { createFrameDecoder, writeFrame } from "./stdioFrames.mts";

const log = (line: string): void => {
  process.stderr.write(`[stdio-channel-host] ${line}\n`);
};

const messages = new Emitter<VSBuffer>();
const protocol: IMessagePassingProtocol = {
  send: (buffer) => writeFrame(process.stdout, buffer.buffer),
  onMessage: messages.event,
};

process.stdin.on("data", createFrameDecoder((frame) => messages.fire(VSBuffer.wrap(frame))));

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
