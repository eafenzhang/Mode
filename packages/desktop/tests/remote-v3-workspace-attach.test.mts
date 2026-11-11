import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

// v3 远程工作区挂接回归：真实 HTTP/WS 服务端 + 假控制面，
// 钉住 capability 票据、`/ws/host` 隧道帧格式与 Initialize 时序。
test("v3 workspace attach：隧道首条 RPC 在超时内完成", async () => {
  const fixture = fileURLToPath(
    new URL("./fixtures/remote-v3-workspace-attach.ts", import.meta.url),
  );
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

  assert.equal(exitCode, 0, `夹具失败（隧道或 Initialize 异常）：\n${output}`);
  assert.match(output, /V3_WORKSPACE_RPC_OK/, `夹具未完成握手断言：\n${output}`);
});
