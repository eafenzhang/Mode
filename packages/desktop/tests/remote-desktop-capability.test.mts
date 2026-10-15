/**
 * 远程桌面能力位回归。运行：
 * node --test packages/desktop/tests/remote-desktop-capability.test.mts
 * 扩展名 .mts：语言政策新测试禁 .mjs（docs/specs/language-policy.md）。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

test("远程桌面能力位：win32 && kill-switch 未触发 && 被控开关开", async () => {
  const fixture = fileURLToPath(
    new URL("./fixtures/remote-desktop-capability.ts", import.meta.url),
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
  assert.match(output, /REMOTE_DESKTOP_CAPABILITY_OK/u, `夹具未完成断言：\n${output}`);
});
