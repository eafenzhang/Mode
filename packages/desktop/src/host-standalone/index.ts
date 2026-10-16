/**
 * Mode Host sidecar 的**独立进程入口**（无 Electron）。
 *
 * 这是 M3 Task 1 的交付物：Rust 壳 spawn 本文件，用 stdio 长度前缀帧搬运字节，
 * 背后是一个真 `ChannelServer`（`@mode/rpc`）+ 一个**全是 stub** 的服务面。
 * 契约与验收分工：`docs/specs/tauri-renderer-host.md`。
 *
 * 为什么它能替掉 Electron 里的 Host：渲染层要的只是「端口 + `IServiceAccessor` +
 * database-startup 状态流」，而 `packages/desktop/src/host/**` 与 `packages/services/**` 里
 * 只有 `electronPort.ts` 一处 Electron 依赖（M3 计划 DR-M3-1 的取证）。本入口不 import 任何
 * Electron 模块，因此可在纯 Node 下运行。
 *
 * 环境变量（壳负责设置，见 spec 的「环境契约」）：
 * - `MODE_HOST_STARTUP_ID`：本次 Host 实例的启动代 id；省略时按 pid 兜底生成（仅便于裸跑）。
 * - `MODE_DATA_BASE_DIR` / `HOME` / `USERPROFILE`：数据根解析，见 `./dataRoot.js`。
 *
 * stdout **只用于帧**（任何日志都走 stderr），否则字节流会被日志污染。
 */
import { ChannelServer, Emitter, VSBuffer, type IMessagePassingProtocol, type IServerChannel } from "@mode/rpc";

import { resolveDataRoot } from "./dataRoot.js";
import { createStubServiceChannels, serviceChannelNames } from "./stubAccessor.js";
import { createFrameDecoder, isRpcRequestFrame, writeFrame } from "./stdioFrames.js";

/** 诊断频道名：不属于 `ServiceChannels`，只报 Host 自身的身份与进程事实（只读，无副作用）。 */
export const HOST_CHANNEL = "host";

const log = (line: string): void => {
  process.stderr.write(`[mode-host-sidecar] ${line}\n`);
};

const startupId = process.env.MODE_HOST_STARTUP_ID?.trim() || `host-${process.pid}`;
const dataRoot = resolveDataRoot(process.env);

const messages = new Emitter<VSBuffer>();
const protocol: IMessagePassingProtocol = {
  // 出方向：原样写出协议层给的字节（按视图边界，见 ./stdioFrames.js）。
  send: (buffer) => writeFrame(process.stdout, buffer.buffer),
  onMessage: messages.event,
};

let rejectedFrames = 0;
process.stdin.on(
  "data",
  createFrameDecoder(
    (frame) => {
      // 形状校验必须在这里：`ChannelServer` 假设对端只发合法帧，投坏帧会让 Host 进程崩
      // （spike §4.6 的实测崩溃）。坏帧只记账并留痕，绝不进协议层。
      if (!isRpcRequestFrame(frame)) {
        rejectedFrames += 1;
        log(`frame_rejected bytes=${frame.byteLength} firstByte=${frame[0] ?? "(none)"}`);
        return;
      }
      messages.fire(VSBuffer.wrap(frame));
    },
    (reason) => log(`frame_decoder_rejected ${reason}`),
  ),
);

// 壳没了（stdin EOF）就该退出：不留孤儿。壳自己也会主动收摊（见 crates/sidecar-host）。
process.stdin.on("end", () => {
  log("stdin_closed_exit");
  process.exit(0);
});
process.stdin.on("error", (error: Error) => {
  log(`stdin_error ${error.message}`);
});

/** 诊断频道：只报事实，不做任何业务动作（M1 的 smoke 吃过「没法观察渲染层」的亏）。 */
const hostChannel: IServerChannel<string> = {
  async call(_ctx, command) {
    if (command === "describe") {
      return {
        name: "mode-host-sidecar",
        protocolVersion: 1,
        pid: process.pid,
        nodeVersion: process.version,
        platform: process.platform,
        arch: process.arch,
        startupId,
        dataRoot: dataRoot.dataRoot,
        dataRootSource: dataRoot.source,
        rejectedFrames,
        serviceChannels: serviceChannelNames(),
      };
    }
    throw new Error(`unknown command ${command}`);
  },
  listen(_ctx, event) {
    throw new Error(`host channel 没有事件：${event}`);
  },
};

// deferInit = true：先把频道注册齐再 ready()，否则 Initialize 可能抢在 registerChannel 之前
// （与 Task 0 的 Host 替身同一理由）。
const serviceChannels = createStubServiceChannels();
if (serviceChannelNames().includes(HOST_CHANNEL)) {
  // 频道名冲突会让诊断频道被服务面顶掉，属于配置错误：立刻失败，不留半可用状态。
  throw new Error(`诊断频道名 ${HOST_CHANNEL} 与 ServiceChannels 冲突`);
}

const server = new ChannelServer(protocol, "mode-host-sidecar", 1000, true);
for (const { channelName, channel } of serviceChannels) {
  server.registerChannel(channelName, channel);
}
server.registerChannel(HOST_CHANNEL, hostChannel);
server.ready();

log(
  `ready startupId=${startupId} dataRoot=${dataRoot.dataRoot} source=${dataRoot.source}` +
    ` channels=${serviceChannels.length + 1} node=${process.version}`,
);
