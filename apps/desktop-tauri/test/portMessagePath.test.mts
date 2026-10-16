/**
 * 覆盖范围：**webview 端口路径的 Node 侧等价物**（自动化，无需 webview）。
 *
 * 它用真 `node:worker_threads.MessageChannel` 造出一对**真 `MessagePort`**，在「页面侧」那端跑
 * 真实 `MessagePortProtocol` + `ChannelClient`（与渲染层 `connectViaMessagePort` 用的是同一对
 * 对象），在「壳侧」那端搭一个 relay 把它桥到 `test/fixtures/stdioChannelHost.mts` 的真
 * `ChannelServer` —— 也就是 harness 里壳所做的事（`invoke` 换成直连 stdio 帧）。
 *
 * 它**能因为端口路径的缺陷而失败**，这是它存在的理由：
 * - 入方向若直接投 `ArrayBuffer`（Tauri raw 响应的形态）而不是 `Uint8Array`，
 *   `MessagePortProtocol` 的 `e.data instanceof Uint8Array` 判定会**静默丢弃**该帧，
 *   `Initialize` 永不达、RPC 永远挂住 —— 第二个用例就是把这个失败现场钉成断言
 *   （spike 实测现场见 `docs/plans/2026-10-16-m3-port-transport-spike.md` §4.3）。
 * - 出方向断言客户端帧确实是 `Uint8Array`（`MessagePortProtocol.send` 发的是 `VSBuffer.buffer`），
 *   所以壳侧不需要为出方向做任何包装。
 *
 * 真 webview 那一半（`window.postMessage` + transfer、`event.source === window`、ACL、投递时序）
 * 无法在 CI 里跑，由 `test/harness/port-transport` 的人工复现覆盖（spike 文档 §3.2）。
 */
import { MessageChannel, type MessagePort as NodeMessagePort } from "node:worker_threads";
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import assert from "node:assert/strict";
import test from "node:test";

import {
  ChannelClient,
  MessagePortProtocol,
  type IChannel,
  type MessagePortLike,
  type MessagePortPayload,
} from "@mode/rpc";

import { createFrameDecoder, writeFrame } from "./fixtures/stdioFrames.mts";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "../../..");
const hostScript = resolve(repoRoot, "apps/desktop-tauri/test/fixtures/stdioChannelHost.mts");

/** 把 Node 的 `MessagePort` 包成 `packages/rpc` 要求的 `MessagePortLike`（形状见 protocol.ts）。 */
function asMessagePortLike(port: NodeMessagePort): MessagePortLike {
  return {
    addEventListener(_type: "message", listener: (event: { data: MessagePortPayload }) => void) {
      port.on("message", (data: MessagePortPayload) => listener({ data }));
    },
    removeEventListener(_type: "message", listener: (event: { data: MessagePortPayload }) => void) {
      port.off("message", listener as unknown as (value: MessagePortPayload) => void);
    },
    postMessage(message: MessagePortPayload) {
      port.postMessage(message);
    },
    start() {
      port.start();
    },
    close() {
      port.close();
    },
  };
}

interface Harness {
  child: ChildProcessWithoutNullStreams;
  /** 页面侧端口（真 `MessagePort`，与渲染层拿到的是同一种对象）。 */
  pagePort: NodeMessagePort;
  /** relay 观测到的「页面 → 壳」帧类型，用于断言出方向不需要包装。 */
  clientFrameTypes: string[];
  stop: () => void;
}

/**
 * 「壳」的等价物：把页面侧端口与 stdio Host 对接。
 * `wrapPortMessage` = harness 里 `__SPIKE_WRAP_PORT_MESSAGE__` 的那个开关。
 */
function startHarness(options: { wrapPortMessage: boolean }): Harness {
  const child = spawn("node", ["--import", "tsx", hostScript], {
    cwd: repoRoot,
    stdio: ["pipe", "pipe", "pipe"],
  });

  const { port1: pagePort, port2: shellPort } = new MessageChannel();
  const clientFrameTypes: string[] = [];

  // 页面 → 壳：MessagePortProtocol.send 发的是 VSBuffer.buffer（Uint8Array），原样写帧。
  shellPort.on("message", (data: MessagePortPayload) => {
    clientFrameTypes.push(Object.prototype.toString.call(data));
    if (!(data instanceof Uint8Array)) return; // flow-control 对象不进 RPC 帧
    writeFrame(child.stdin, data);
  });
  shellPort.start();

  // 壳 → 页面：把 stdio 帧投回端口，形态由 `wrapPortMessage` 决定。
  child.stdout.on(
    "data",
    createFrameDecoder((frame) => {
      if (options.wrapPortMessage) {
        shellPort.postMessage(new Uint8Array(frame));
        return;
      }
      // 失败形态：投**真 ArrayBuffer**，也就是 Tauri raw 响应未被包装时的形态。
      const standalone = frame.buffer.slice(frame.byteOffset, frame.byteOffset + frame.byteLength);
      shellPort.postMessage(standalone);
    }),
  );

  return {
    child,
    pagePort,
    clientFrameTypes,
    stop: () => {
      shellPort.close();
      pagePort.close();
      child.kill();
    },
  };
}

const delay = (ms: number): Promise<void> => new Promise((resolveDelay) => setTimeout(resolveDelay, ms));

test("端口路径：入方向包 Uint8Array 时 Initialize 与 RPC 往返成立", async () => {
  const harness = startHarness({ wrapPortMessage: true });
  try {
    const client = new ChannelClient(new MessagePortProtocol(asMessagePortLike(harness.pagePort)));
    let initializeCount = 0;
    client.onDidInitialize(() => {
      initializeCount += 1;
    });

    const result = await client
      .getChannel<IChannel>("spike")
      .call<{ command: string; echo: { n: number; text: string } }>("ping", {
        n: 41,
        text: "from-page",
      });

    assert.equal(initializeCount, 1, "端口承载了 ChannelServer 的 Initialize");
    assert.equal(result.command, "ping");
    assert.deepEqual(result.echo, { n: 41, text: "from-page" });
    assert.deepEqual(
      harness.clientFrameTypes,
      ["[object Uint8Array]"],
      "出方向的客户端帧本来就是 Uint8Array，壳侧不需要为它做包装",
    );
    client.dispose();
  } finally {
    harness.stop();
  }
});

test("陷阱：入方向直接把 ArrayBuffer 投进端口会被 MessagePortProtocol 静默丢弃", async () => {
  const harness = startHarness({ wrapPortMessage: false });
  try {
    const client = new ChannelClient(new MessagePortProtocol(asMessagePortLike(harness.pagePort)));
    let initializeCount = 0;
    client.onDidInitialize(() => {
      initializeCount += 1;
    });

    // 头一帧就是 Host 的 Initialize；它在 Host 启动时就已经发出，所以等到这里必然已经到达。
    const ping = client.getChannel<IChannel>("spike").call("ping", { n: 41 });
    const settled = await Promise.race([
      delay(1_500).then(() => "pending" as const),
      ping.then(() => "resolved" as const, () => "rejected" as const),
    ]);

    assert.equal(initializeCount, 0, "ArrayBuffer 帧被丢弃：Initialize 没有到达 ChannelClient");
    assert.equal(settled, "pending", "请求既不成功也不报错，就是静默丢帧的症状（挂住）");
    client.dispose();
  } finally {
    harness.stop();
  }
});
