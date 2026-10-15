/**
 * 远程桌面 main↔host 消息契约与双端镜像回归（漂移 + 分流不误吞）。
 * 运行：node --test packages/desktop/tests/remote-desktop-host-messages.test.mts
 * 扩展名 .mts：语言政策新测试禁 .mjs（docs/specs/language-policy.md）；
 * 夹具走 `node --import tsx`（与 remote-desktop-rdp.test.mts 同一口径）。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

test("远程桌面消息契约：两份镜像逐条一致 + 已删消息族不复活 + 分流不误吞", async () => {
  const fixture = fileURLToPath(
    new URL("./fixtures/remote-desktop-host-messages.ts", import.meta.url),
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
  assert.match(output, /REMOTE_DESKTOP_HOST_MESSAGES_OK/u, `夹具未完成断言：\n${output}`);
});
