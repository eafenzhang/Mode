/**
 * 远程桌面（LAN）host 侧纯逻辑回归：输入换算与批量、编码能力缓存持久化。
 * 运行：node --test packages/desktop/tests/remote-desktop.test.mts
 * 扩展名 .mts：语言政策新测试禁 .mjs（docs/specs/language-policy.md）；
 * 夹具走 `node --import tsx`（与 remote-desktop-capability.test.mts 同一口径）。
 *
 * Electron 线 EOL（docs/specs/legacy-electron-eol.md）后，main 侧会话状态机与
 * main↔host 消息镜像夹具随 main 一起删除；被控协调器由
 * remote-desktop-controlled-coordinator.test.mts 覆盖。
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

test("输入换算与批量：多屏负原点坐标、滚轮刻度、按帧聚合", async () => {
  await runFixture("remote-desktop-input", /REMOTE_DESKTOP_INPUT_OK/u);
});

test("编码能力缓存持久化：写读回环、损坏降级、白名单收敛", async () => {
  await runFixture("remote-desktop-encoder-capability", /REMOTE_DESKTOP_ENCODER_CAPABILITY_OK/u);
});
