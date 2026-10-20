import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

// 公网配对回归（docs/specs/lan-remote-public-endpoints.md）：
// 配对成功后 meta 同时保存 LAN 与公网端点；非 HTTPS 公网地址在发请求前拒绝。
test("pairLanPeer：公网端点随配对落 meta，http 公网地址被拒绝", async () => {
  const fixture = fileURLToPath(new URL("./fixtures/lan-public-pair.ts", import.meta.url));
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

  const exitCode = await new Promise<number | null>((resolve, reject) => {
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

  assert.equal(exitCode, 0, `夹具失败（公网端点未落 meta 或 http 未被拒绝）：\n${output}`);
  assert.match(output, /LAN_PUBLIC_PAIR_OK/u, `夹具未完成断言：\n${output}`);
});
