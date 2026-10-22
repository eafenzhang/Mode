/**
 * TUI PTY harness（场景清单见 `docs/plans/2026-10-18-m3-tui-status.md`）。
 *
 * 真 `runTui` 跑在 `node-pty` 里（stdin/stdout 都是 TTY），运行时用夹具的
 * `submitPrompt` 假实现驱动——不接模型、不读凭据。当前覆盖：
 *
 * - A1 冷启动：首屏渲染出产品名与输入提示；
 * - B1 文本增量：提交后两次 `text_delta` 的文本都出现在屏幕上。
 *
 * 退出链路不在本用例里：会话屏的 Ctrl-C 带防误退/中断流语义，发行形态的
 * 键盘退出已由 `scripts/mode-distribution-smoke.mjs` 覆盖；这里用 `terminal.kill()` 收尾，
 * 避免把「按键时序」混进渲染场景。
 *
 * 其余场景按同一骨架追加：改夹具的 `submitPrompt` 事件脚本 + 这里加一条断言。
 */
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import test from "node:test";

import * as pty from "node-pty";

const here = dirname(fileURLToPath(import.meta.url));
const packageRoot = resolve(here, "..");
const fixture = resolve(here, "fixtures/tuiPtyFixture.mts");

const delay = (ms: number): Promise<void> =>
  new Promise((resolveDelay) => setTimeout(resolveDelay, ms));

/** 去掉 ANSI 控制序列：TUI 用备用屏与光标移动，原始输出里会夹带转义。 */
function stripAnsi(value: string): string {
  return value
    .replace(/\u001b\[[0-9;?]*[ -/]*[@-~]/g, "")
    .replace(/\u001b\][^\u0007]*\u0007/g, "")
    .replace(/\r/g, "");
}

async function until(predicate: () => boolean, label: string, snapshot: () => string, ms = 15_000) {
  const startedAt = Date.now();
  while (!predicate()) {
    if (Date.now() - startedAt > ms) {
      throw new Error(`${label} 超时（${ms}ms）\n--- screen ---\n${stripAnsi(snapshot())}`);
    }
    await delay(50);
  }
}

test("TUI PTY：冷启动渲染 → 提交后文本增量上屏", async () => {
  const terminal = pty.spawn(process.execPath, ["--import", "tsx", fixture], {
    name: "xterm-256color",
    cols: 110,
    rows: 32,
    cwd: packageRoot,
    env: {
      ...process.env,
      TERM: "xterm-256color",
      NO_COLOR: "1",
    },
  });
  let output = "";
  terminal.onData((data) => {
    output += data;
  });

  try {
    // A1：首屏包含模式标签与输入提示（夹具用 en-US；启动屏不经过 loadStartupOptions）。
    await until(
      () => /Build/.test(stripAnsi(output)) && /Type a prompt|Ask Mode/i.test(stripAnsi(output)),
      "TUI 冷启动首屏",
      () => output,
    );

    // B1：输入并提交，夹具按脚本推两次 text_delta。
    terminal.write("hello pty\r");
    await until(
      () => stripAnsi(output).includes("PTY_SUBMIT_1"),
      "submitPrompt 调用",
      () => output,
    );
    await until(
      () => stripAnsi(output).includes("PTY_STREAM_ONE"),
      "第一个文本增量上屏",
      () => output,
    );
    await until(
      () => stripAnsi(output).includes("PTY_STREAM_TWO"),
      "第二个文本增量上屏",
      () => output,
    );
    assert.doesNotMatch(stripAnsi(output), /ERR_MODULE_NOT_FOUND|Cannot find module/u);
  } finally {
    terminal.kill();
  }
});
