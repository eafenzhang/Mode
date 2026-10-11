/**
 * 远程桌面（LAN）desktop 侧纯逻辑回归：会话状态机、输入换算与批量、main↔host 消息契约镜像。
 * 运行：node --test packages/desktop/tests/remote-desktop.test.mts
 * 扩展名 .mts：语言政策新测试禁 .mjs（docs/specs/language-policy.md）；
 * 夹具走 `node --import tsx`（与 remote-desktop-capability.test.mts 同一口径）。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

async function runFixture(name: string, okMarker: RegExp): Promise<void> {
  const fixture = fileURLToPath(new URL(`./fixtures/${name}.ts`, import.meta.url));
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
  assert.match(output, okMarker, `夹具未完成断言：\n${output}`);
}

test("被控会话状态机：请求/同意/收口与单会话互斥", async () => {
  await runFixture("remote-desktop-session", /REMOTE_DESKTOP_SESSION_OK/u);
});

test("输入换算与批量：多屏负原点坐标、滚轮刻度、按帧聚合", async () => {
  await runFixture("remote-desktop-input", /REMOTE_DESKTOP_INPUT_OK/u);
});

test("main↔host 消息契约：双端镜像、严格校验与准入判定", async () => {
  await runFixture("remote-desktop-host-messages", /REMOTE_DESKTOP_HOST_MESSAGES_OK/u);
});
