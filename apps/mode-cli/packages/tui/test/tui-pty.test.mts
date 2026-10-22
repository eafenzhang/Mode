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
 * - B5 中断：Esc 中止进行中的回合；
 * - C3 审批：面板出现，默认 Deny 确认后返回 deny；
 * - C4 提问：问题面板、作答与 Review 提交；
 * - D1 工具卡片：running → completed 状态都上屏；
 * - E2 斜杠命令：输入 `/` 出现补全，Tab 补全后提交；
 * - E3 模式切换：Shift+Tab 调用客户端的 setMode（标记写入，界面标签不在断言面）。
 *
 * 退出链路不在本用例里：会话屏的 Ctrl-C 带防误退/中断流语义，发行形态的
 * 键盘退出已由 `scripts/mode-distribution-smoke.mjs` 覆盖；这里用 `terminal.kill()` 收尾。
 *
 * 其余场景按同一骨架追加：在夹具里加 `PTY_SCENARIO` 分支 + 这里加一条断言。
 */
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
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

function startTui(scenario: string, extraEnv: Record<string, string> = {}): TuiSession {
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
      ...extraEnv,
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

test("TUI PTY B5：Esc 中止进行中的回合", async () => {
  const resultDir = mkdtempSync(join(tmpdir(), "mode-pty-interrupt-"));
  const resultFile = join(resultDir, "result.txt");
  const session = startTui("interrupt", { PTY_RESULT_FILE: resultFile });
  try {
    await waitForReady(session);
    session.write("long running task\r");
    // 提交是否进到运行时以结果文件为准（屏幕标记会被重绘吃掉）。
    const startedDeadline = Date.now() + 8_000;
    let started = false;
    while (Date.now() < startedDeadline) {
      try {
        if (readFileSync(resultFile, "utf8").includes("PTY_INTERRUPT_SUBMITTED")) {
          started = true;
          break;
        }
      } catch {
        // 文件尚未创建：继续轮询。
      }
      await delay(200);
    }
    assert.ok(started, `回合未开始\n--- screen ---\n${session.screen()}`);
    await delay(300);
    session.write("\u001b");

    const deadline = Date.now() + 8_000;
    let aborted = false;
    while (Date.now() < deadline) {
      try {
        if (readFileSync(resultFile, "utf8").includes("PTY_ABORTED")) {
          aborted = true;
          break;
        }
      } catch {
        // 文件尚未创建：继续轮询。
      }
      await delay(200);
    }
    assert.ok(aborted, `Esc 未中止进行中的回合\n--- screen ---\n${session.screen()}`);
  } finally {
    session.stop();
    rmSync(resultDir, { recursive: true, force: true });
  }
});

test("TUI PTY C3：审批面板出现，默认 Deny 确认", async () => {
  const resultDir = mkdtempSync(join(tmpdir(), "mode-pty-approval-"));
  const resultFile = join(resultDir, "result.txt");
  const session = startTui("approval", { PTY_RESULT_FILE: resultFile });
  try {
    await waitForReady(session);
    session.write("run it\r");
    await session.waitForSubstring("Approval required: Bash", "审批面板出现");
    await session.waitForSubstring("Up/Down choose", "审批面板提示行");
    // 面板默认选中 Deny（安全默认），Enter 确认。首个按键在负载下偶发丢失，
    // 改为「轮询结果文件 + 期间周期性重发 Enter」，直到夹具记录 deny。
    const deadline = Date.now() + 8_000;
    let decided = false;
    while (Date.now() < deadline) {
      session.write("\r");
      await delay(300);
      try {
        if (readFileSync(resultFile, "utf8").includes("PTY_APPROVAL_D")) {
          decided = true;
          break;
        }
      } catch {
        // 文件尚未创建：继续重试。
      }
    }
    assert.equal(decided, true, `审批未在期限内返回 deny\n--- screen ---\n${session.screen()}`);
  } finally {
    session.stop();
    rmSync(resultDir, { recursive: true, force: true });
  }
});

test("TUI PTY C4：问题面板渲染、作答并提交", async () => {
  const resultDir = mkdtempSync(join(tmpdir(), "mode-pty-question-"));
  const resultFile = join(resultDir, "result.txt");
  const session = startTui("question", { PTY_RESULT_FILE: resultFile });
  try {
    await waitForReady(session);
    session.write("ask\r");
    await session.waitForSubstring("Question 1/1: Approach", "问题面板标题");
    await session.waitForSubstring("Pick one?", "问题文本");
    await session.waitForSubstring("Approach A", "选项标签");

    // Enter 作答 → Review；Review 再 Enter 提交（decision: modify）。
    // 首个按键在负载下可能丢失，与 C3 同样用轮询重发。
    const deadline = Date.now() + 10_000;
    let sawReview = false;
    let submitted = false;
    while (Date.now() < deadline) {
      session.write("ask".slice(0, 0) + "\r");
      await delay(300);
      if (!sawReview && session.screen().includes("Review answers")) {
        sawReview = true;
      }
      try {
        if (readFileSync(resultFile, "utf8").includes("PTY_APPROVAL_M")) {
          submitted = true;
          break;
        }
      } catch {
        // 文件尚未创建：继续重试。
      }
    }
    assert.ok(sawReview, `未进入 Review\n--- screen ---\n${session.screen()}`);
    assert.ok(submitted, `问题作答未提交（decision: modify）\n--- screen ---\n${session.screen()}`);
  } finally {
    session.stop();
    rmSync(resultDir, { recursive: true, force: true });
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

test("TUI PTY E3：Shift+Tab 触发模式切换", async () => {
  const session = startTui("stream");
  try {
    await waitForReady(session);
    // 初始 Build；切换顺序 plan → build → edit → yolo，所以下一档是 edit。
    for (const sequence of ["\u001b[9;2u", "\u001b[1;2Z", "\u001b[Z"]) {
      session.write(sequence);
      await delay(300);
      if (/PTY_MODE_/u.test(session.screen())) break;
    }
    await session.waitForSubstring("PTY_MODE_edit", "setMode 收到 edit");
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
