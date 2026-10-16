/**
 * 覆盖范围：**Host 侧的 stdio 帧 + 真 `ChannelServer`/`ChannelClient` 的 RPC 语义**。
 *
 * 它 spawn `test/fixtures/stdioChannelHost.mts`（真 `ChannelServer`），用测试自己搭的
 * `IMessagePassingProtocol` 在 stdio 帧上做真实 Initialize 与真实 promise 往返。
 * **它不覆盖 webview 端口路径**（`window.postMessage` + transfer、`MessagePortProtocol` 的
 * 收帧类型、`DatabaseStartupAdmission` 的准入）——那一半由
 * `test/portMessagePath.test.mts`（Node 侧真实 `MessagePort`，自动化）与
 * `test/harness/port-transport`（真 webview，人工，见 spike 文档 §3.2）覆盖。
 *
 * 运行：`pnpm --filter @mode/desktop-tauri test`
 */
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import assert from "node:assert/strict";
import test from "node:test";

import { ChannelClient, Emitter, VSBuffer, type IChannel, type IMessagePassingProtocol } from "@mode/rpc";

import { createFrameDecoder, writeFrame } from "./fixtures/stdioFrames.mts";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "../../..");
const hostScript = resolve(repoRoot, "apps/desktop-tauri/test/fixtures/stdioChannelHost.mts");

interface StdioHost {
  child: ChildProcessWithoutNullStreams;
  protocol: IMessagePassingProtocol;
  stderr: () => string;
}

/** 与 harness 的壳用同一套帧格式（共用 `writeFrame` / `createFrameDecoder`）。 */
function startStdioHost(): StdioHost {
  const child = spawn("node", ["--import", "tsx", hostScript], {
    cwd: repoRoot,
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
  child.stdout.on("data", createFrameDecoder((frame) => messages.fire(VSBuffer.wrap(frame))));

  return { child, protocol, stderr: () => stderrText };
}

test("真实 ChannelServer 的 Initialize 与一次 promise 往返可以走 stdio 帧", async () => {
  const host = startStdioHost();
  try {
    const client = new ChannelClient(host.protocol);
    const initialized = new Promise<void>((resolveInit, rejectInit) => {
      const timer = setTimeout(
        () => rejectInit(new Error(`没有收到 Initialize；host stderr:\n${host.stderr()}`)),
        10_000,
      );
      client.onDidInitialize(() => {
        clearTimeout(timer);
        resolveInit();
      });
    });

    const result = await client
      .getChannel<IChannel>("spike")
      .call<{ command: string; echo: { n: number; text: string }; hostPid: number }>("ping", {
        n: 41,
        text: "spike",
      });

    await initialized;
    assert.equal(result.command, "ping");
    assert.deepEqual(result.echo, { n: 41, text: "spike" });
    assert.ok(result.hostPid > 0, "返回值来自 Node 侧进程");
    client.dispose();
  } finally {
    host.child.kill();
  }
});

test("未知频道/命令的失败会作为 RPC 错误回到调用方，而不是挂住", async () => {
  const host = startStdioHost();
  try {
    const client = new ChannelClient(host.protocol);
    await assert.rejects(
      client.getChannel<IChannel>("spike").call("does-not-exist"),
      /unknown command does-not-exist/,
    );
    client.dispose();
  } finally {
    host.child.kill();
  }
});
