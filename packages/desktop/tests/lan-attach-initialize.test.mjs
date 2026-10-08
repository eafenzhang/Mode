import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

// 局域网 attach 的 Initialize 时序回归：
// 服务端只在建连瞬间发一帧 Initialize，客户端（lanRemoteAttach）的 message 监听
// 必须在 await open 之前挂载，否则该帧被静默丢弃，ChannelClient 永久排队——
// 表现为连接成功但目录浏览器永远「加载中…」（无报错、无日志）。
// 失效模式：夹具首条 RPC 超时，进程非 0 退出。
test("LAN attach：Initialize 帧不因监听晚挂丢失，首条 RPC 在超时内完成", async () => {
  const fixture = fileURLToPath(new URL("./fixtures/lan-attach-initialize.ts", import.meta.url));
  const repoRoot = fileURLToPath(new URL("../../../", import.meta.url));
  const child = spawn(process.execPath, ["--import", "tsx", fixture], {
    cwd: repoRoot,
    env: process.env,
    stdio: ["ignore", "pipe", "pipe"],
  });

  let output = "";
  child.stdout.on("data", (chunk) => {
    output += String(chunk);
  });
  child.stderr.on("data", (chunk) => {
    output += String(chunk);
  });

  const exitCode = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      child.kill();
      reject(new Error(`夹具进程 30s 未退出\n${output}`));
    }, 30_000);
    child.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.once("close", (code) => {
      clearTimeout(timer);
      resolve(code);
    });
  });

  assert.equal(exitCode, 0, `夹具失败（Initialize 丢失或 RPC 悬空）：\n${output}`);
  assert.match(output, /LAN_ATTACH_RPC_OK/, `夹具未完成握手断言：\n${output}`);
});
