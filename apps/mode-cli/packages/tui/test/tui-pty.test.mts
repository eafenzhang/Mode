/**
 * TUI PTY harness（场景清单见 `docs/plans/2026-10-18-m3-tui-status.md`）。
 *
 * 真 `runTui` 跑在 `node-pty` 里（stdin/stdout 都是 TTY），运行时用夹具的
 * `submitPrompt` 假实现驱动——不接模型、不读凭据。已覆盖：
 *
 * - A1 冷启动：首屏渲染出模式标签与输入提示；
 * - A3 一轮对话完成：`turn_complete` 后终态响应上屏；
 * - A5 排队输入：busy 时第二次提交进入队列面板；
 * - B1 文本增量：两次 `text_delta` 都出现；
 * - B2 工具输入增量：不进入正文，最终工具卡完成；
 * - B3 思考块：`reasoning_delta` 以折叠块呈现；
 * - C3 审批：面板出现，默认 Deny 确认后返回 deny；
 * - D1 工具卡片：running → completed 状态都上屏；
 * - E2 斜杠命令：输入 `/` 出现补全，Tab 补全后提交。
 *
 * 退出链路不在本用例里：会话屏的 Ctrl-C 带防误退/中断流语义，发行形态的
 * 键盘退出已由 `scripts/mode-distribution-smoke.mjs` 覆盖；这里用 `terminal.kill()` 收尾。
 *
 * 其余场景按同一骨架追加：在夹具里加 `PTY_SCENARIO` 分支 + 这里加一条断言。
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

interface TuiSession {
  terminal: pty.IPty;
  write(data: string): void;
  waitFor(pattern: RegExp, label: string, ms?: number): Promise<void>;
  waitForSubstring(text: string, label: string, ms?: number): Promise<void>;
  screen(): string;
  stop(): void;
}

function startTui(scenario: string): TuiSession {
  const terminal = pty.spawn(process.execPath, ["--import", "tsx", fixture], {
    name: "xterm-256color",
    cols: 110,
    rows: 32,
    cwd: packageRoot,
    env: {
      ...process.env,
      TERM: "xterm-256color",
      NO_COLOR: "1",
      PTY_SCENARIO: scenario,
    },
  });
  let output = "";
  terminal.onData((data) => {
    output += data;
  });
  const waitFor = async (pattern: RegExp, label: string, ms = 15_000): Promise<void> => {
    const startedAt = Date.now();
    while (!pattern.test(stripAnsi(output))) {
      if (Date.now() - startedAt > ms) {
        throw new Error(`${label} 超时（${ms}ms）\n--- screen ---\n${stripAnsi(output)}`);
      }
      await delay(50);
    }
  };
  return {
    terminal,
    write: (data) => terminal.write(data),
    waitFor,
    waitForSubstring: (text, label, ms) =>
      waitFor(new RegExp(text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "u"), label, ms),
    screen: () => stripAnsi(output),
    stop: () => terminal.kill(),
  };
}

async function waitForReady(session: TuiSession): Promise<void> {
  // en-US：Windows PTY 主端可能按系统代码页解码，中文会变乱码；只用 ASCII 断言。
  await session.waitFor(/Build/u, "模式标签");
  await session.waitFor(/Type a prompt|Ask Mode/iu, "输入提示");
}

test("TUI PTY A1+B1：冷启动渲染 → 提交后文本增量上屏", async () => {
  const session = startTui("stream");
  try {
    await waitForReady(session);
    session.write("hello pty\r");
    await session.waitForSubstring("PTY_SUBMIT_1", "submitPrompt 调用");
    await session.waitForSubstring("PTY_STREAM_ONE", "第一个文本增量上屏");
    await session.waitForSubstring("PTY_STREAM_TWO", "第二个文本增量上屏");
    assert.doesNotMatch(session.screen(), /ERR_MODULE_NOT_FOUND|Cannot find module/u);
  } finally {
    session.stop();
  }
});

test("TUI PTY A3：turn_complete 的终态响应上屏", async () => {
  const session = startTui("turn");
  try {
    await waitForReady(session);
    session.write("finish this turn\r");
    await session.waitForSubstring("PTY_TURN_TEXT", "回合文本上屏");
    await session.waitForSubstring("PTY_TURN_DONE", "turn_complete 的响应上屏");
  } finally {
    session.stop();
  }
});

test("TUI PTY A5：busy 时第二次提交进入队列面板", async () => {
  const session = startTui("queue");
  try {
    await waitForReady(session);
    session.write("first turn\r");
    await session.waitForSubstring("PTY_SUBMIT_1", "第一次提交");
    session.write("queued prompt\r");
    await session.waitForSubstring("PTY_SEND_INPUT_1", "busy 输入走 sendInput");
    await session.waitForSubstring("Queue (1)", "排队面板出现");
    await session.waitForSubstring("queued prompt", "排队输入内容上屏");
  } finally {
    session.stop();
  }
});

test("TUI PTY B2：工具输入增量不进入正文，最终工具卡完成", async () => {
  const session = startTui("toolinput");
  try {
    await waitForReady(session);
    session.write("prepare a tool\r");
    await session.waitForSubstring("Tool Bash completed", "工具卡完成");
    assert.doesNotMatch(
      session.screen(),
      /PTY_TOOL_INPUT_DELTA/u,
      "工具输入增量是 JSON 参数，不能画进助手正文",
    );
  } finally {
    session.stop();
  }
});

test("TUI PTY B3：reasoning 以折叠思考块呈现", async () => {
  const session = startTui("reasoning");
  try {
    await waitForReady(session);
    session.write("think first\r");
    // 思考块默认折叠，只渲染 `+ Thinking...` / `+ Thought`；正文仍单独渲染。
    await session.waitFor(/\+ (?:Thinking\.\.\.|Thought)/u, "折叠思考块");
    await session.waitForSubstring("PTY_REASONING_ANSWER", "正文回答上屏");
  } finally {
    session.stop();
  }
});

test("TUI PTY C3：审批面板出现，默认 Deny 确认", async () => {
  const session = startTui("approval");
  try {
    await waitForReady(session);
    session.write("run it\r");
    await session.waitForSubstring("Approval required: Bash", "审批面板出现");
    await session.waitForSubstring("Up/Down choose", "审批面板提示行");
    // 面板绘制与键盘处理器读到的 approvalQueue 之间有一个渲染周期；稍等再确认。
    await delay(400);
    // 面板默认选中 Deny（安全默认），Enter 确认。并发负载下首个按键偶发丢失，补一次。
    session.write("\r");
    await delay(500);
    if (!session.screen().includes("PTY_APPROVAL_D")) {
      session.write("\r");
    }
    // 夹具用短码避免 TUI 重绘裁掉长标记：A=allow_once / B=allow_project / D=deny。
    await session.waitForSubstring("PTY_APPROVAL_D", "审批返回 deny");
  } finally {
    session.stop();
  }
});

test("TUI PTY E2：输入 / 出现补全，Tab 补全后提交", async () => {
  const session = startTui("stream");
  try {
    await waitForReady(session);
    session.write("/");
    await session.waitForSubstring("Commands", "斜杠建议面板");
    await session.waitForSubstring("/help", "命令候选");
    session.write("\t");
    await delay(120);
    session.write("now\r");
    await session.waitForSubstring("PTY_PROMPT_HELP", "补全后的 /help 进入提交");
  } finally {
    session.stop();
  }
});

test("TUI PTY D1：工具卡片 running → completed 状态上屏", async () => {
  const session = startTui("tool");
  try {
    await waitForReady(session);
    session.write("run a tool\r");
    await session.waitForSubstring("Tool Bash running", "工具 running 状态");
    await session.waitForSubstring("Tool Bash completed", "工具 completed 状态");
  } finally {
    session.stop();
  }
});
