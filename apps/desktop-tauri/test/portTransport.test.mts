/**
 * M3 Task 0 端口传输 spike 留下的**可回归验证**（无 GUI，可进 CI）：
 *
 * 用真实 `packages/rpc` 的 `ChannelClient` 与 `test/fixtures/stdioChannelHost.ts`（真实 `ChannelServer`）
 * 在 stdio 长度前缀帧上做一次真实 RPC——这正是 Tauri 壳与 Host 之间、以及壳与页面之间要搬运的那条
 * 链路的 Node 侧一半。结论与 webview 侧的实测见
 * `docs/plans/2026-10-16-m3-port-transport-spike.md`。
 *
 * 运行：`pnpm --filter @mode/desktop-tauri test`
 */
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import assert from "node:assert/strict";
import test from "node:test";

import { ChannelClient } from "../../../packages/rpc/src/channelClient.js";
import { VSBuffer } from "../../../packages/rpc/src/buffer.js";
import { Emitter } from "../../../packages/rpc/src/foundation.js";
import type { IChannel } from "../../../packages/rpc/src/channels.shared.js";
import type { IMessagePassingProtocol } from "../../../packages/rpc/src/protocol.js";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "../../..");
const hostScript = resolve(repoRoot, "apps/desktop-tauri/test/fixtures/stdioChannelHost.ts");

interface StdioHost {
  child: ChildProcessWithoutNullStreams;
  protocol: IMessagePassingProtocol;
  stderr: () => string;
}

/** 与 Rust 壳用的同一套帧格式：[4 字节大端长度][payload]。 */
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
    send: (buffer) => {
      const header = Buffer.alloc(4);
      header.writeUInt32BE(buffer.buffer.byteLength, 0);
      child.stdin.write(header);
      child.stdin.write(Buffer.from(buffer.buffer));
    },
    onMessage: messages.event,
  };

  let pending = Buffer.alloc(0);
  child.stdout.on("data", (chunk: Buffer) => {
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
