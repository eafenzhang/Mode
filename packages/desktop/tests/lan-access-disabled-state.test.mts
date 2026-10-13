/**
 * 局域网访问「监听未开启时状态读取」回归。运行：
 * node --test packages/desktop/tests/lan-access-disabled-state.test.mts
 * 扩展名 .mts：语言政策新测试禁 .mjs（docs/specs/language-policy.md）。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

test("局域网状态读取：关开关仍回配对记录（含远程桌面授权）且回帖 schema 可解析", async () => {
  const fixture = fileURLToPath(
    new URL("./fixtures/lan-access-disabled-state.ts", import.meta.url),
  );
  const repoRoot = fileURLToPath(new URL("../../../", import.meta.url));
  const child = spawn(process.execPath, ["--import", "tsx", fixture], {
    cwd: repoRoot,
    env: process.env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  let output = "";
  child.stdout.on("data", (chunk) => (output += String(chunk)));
  child.stderr.on("data", (chunk) => (output += String(chunk)));
  const exitCode = await new Promise<number>((resolve, reject) => {
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
      resolve(code ?? 1);
    });
  });
  assert.equal(exitCode, 0, `夹具失败：\n${output}`);
  assert.match(output, /LAN_ACCESS_DISABLED_STATE_OK/u, `夹具未完成断言：\n${output}`);
});
