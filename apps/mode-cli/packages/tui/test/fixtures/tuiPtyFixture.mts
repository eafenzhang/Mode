/**
 * PTY harness 的 TUI 夹具：真 `runTui` + 假运行时。
 *
 * 场景由 `PTY_SCENARIO` 选择；每次 `submitPrompt` 按场景脚本推事件或等待审批，
 * 不接模型、不读凭据、不写数据根。PTY 由测试侧 `node-pty` 提供（stdin/stdout 都是 TTY）。
 *
 * 测试可见标记（写到 stderr，与 TUI 共用同一个 PTY）：
 * - `PTY_SUBMIT_<n>`：第 n 次提交；
 * - `PTY_APPROVAL_<decision>`：审批返回的决策；
 * - `PTY_STREAM_*` / `PTY_TURN_*` / `PTY_THOUGHT_*` / 工具名等则经真实的 TUI 渲染路径显示。
 */
import { randomUUID } from "node:crypto";
import { appendFileSync } from "node:fs";

import { SessionEventType, type SessionEvent } from "@mode/contracts";
import { runTui } from "../../src/index.js";

const scenario = process.env.PTY_SCENARIO?.trim() || "stream";
const sessionId = "pty-fixture-session";
const traceId = "pty-fixture-trace";
const assistantMessageId = "pty-assistant-message";
let sequence = 0;
let submitCount = 0;

function event(type: SessionEvent["type"], payload: unknown, turnId?: string): SessionEvent {
  sequence += 1;
  return {
    id: randomUUID(),
    sessionId,
    ...(turnId ? { turnId } : {}),
    timestamp: new Date(),
    traceId,
    sequenceNumber: sequence,
    type,
    payload,
  };
}

const delay = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * 控制信号旁路：PTY 屏幕会被 TUI 重绘覆盖，长标记也可能被裁掉。
 * 测试可通过 `PTY_RESULT_FILE` 指定一个文件，夹具把关键结果追加进去，避免靠屏幕轮询。
 */
const resultFile = process.env.PTY_RESULT_FILE?.trim();
function recordControl(line: string): void {
  if (!resultFile) return;
  try {
    appendFileSync(resultFile, `${line}\n`);
  } catch {
    // 结果文件写失败不影响 TUI 本身；stderr 仍保留同样的标记供人工排障。
  }
}

type SubmitOptions = {
  onEvent?: (event: SessionEvent) => unknown;
  requestPermission?: (request: unknown, options?: unknown) => Promise<{ decision: string }>;
};

function promptTextOf(prompt: unknown): string {
  if (typeof prompt === "string") return prompt;
  if (
    prompt &&
    typeof prompt === "object" &&
    typeof (prompt as { text?: unknown }).text === "string"
  ) {
    return (prompt as { text: string }).text;
  }
  return "";
}

let sendInputCount = 0;

async function submit(prompt: unknown, options: SubmitOptions) {
  submitCount += 1;
  process.stderr.write(`PTY_SUBMIT_${submitCount}\n`);
  if (promptTextOf(prompt).trim().startsWith("/help")) {
    // E2：补全后的命令确实进入提交入口。
    process.stderr.write("PTY_PROMPT_HELP\n");
  }
  const emit = (type: SessionEvent["type"], payload: Record<string, unknown>, turnId?: string) => {
    void options.onEvent?.(event(type, payload, turnId));
  };
  const streamText = (text: string) =>
    emit(SessionEventType.ModelStreaming, {
      kind: "text_delta",
      delta: text,
      done: false,
      assistantMessageId,
    });

  if (scenario === "turn") {
    emit(SessionEventType.TurnStarted, {}, "pty-turn-1");
    streamText("PTY_TURN_TEXT ");
    await delay(100);
    emit(SessionEventType.TurnComplete, {
      response: "PTY_TURN_DONE",
      usage: { inputTokens: 5, outputTokens: 3 },
    });
    return { response: "PTY_TURN_DONE" };
  }

  if (scenario === "reasoning") {
    emit(SessionEventType.ModelStreaming, {
      kind: "reasoning_start",
      delta: "",
      done: false,
      assistantMessageId,
    });
    emit(SessionEventType.ModelStreaming, {
      kind: "reasoning_delta",
      delta: "PTY_THOUGHT_ONE",
      done: false,
      assistantMessageId,
    });
    emit(SessionEventType.ModelStreaming, {
      kind: "reasoning_end",
      delta: "",
      done: false,
      assistantMessageId,
    });
    streamText("PTY_REASONING_ANSWER");
    await delay(150);
    return { response: "PTY_REASONING_ANSWER" };
  }

  if (scenario === "toolinput") {
    emit(SessionEventType.ModelStreaming, {
      kind: "tool_input_start",
      toolName: "Bash",
      delta: "",
      done: false,
      assistantMessageId,
    });
    // 每个增量之间留一帧：状态行在 burst 里会被后续状态覆盖，来不及绘制。
    await delay(120);
    emit(SessionEventType.ModelStreaming, {
      kind: "tool_input_delta",
      toolName: "Bash",
      delta: "PTY_TOOL_INPUT_DELTA",
      done: false,
      assistantMessageId,
    });
    await delay(120);
    emit(SessionEventType.ModelStreaming, {
      kind: "tool_input_end",
      toolName: "Bash",
      delta: "",
      done: false,
      assistantMessageId,
    });
    await delay(120);
    emit(SessionEventType.ModelStreaming, {
      kind: "tool_call",
      toolName: "Bash",
      toolCallId: "tool-input-1",
      delta: "",
      done: false,
      assistantMessageId,
    });
    await delay(120);
    emit(SessionEventType.ToolCallScheduled, {
      toolCallId: "tool-input-1",
      toolName: "Bash",
      input: { command: "echo from-input" },
      schedule: { kind: "immediate" },
    });
    await delay(120);
    emit(SessionEventType.ToolCallStarted, {
      toolCallId: "tool-input-1",
      toolName: "Bash",
      startedAt: new Date(),
    });
    await delay(120);
    emit(SessionEventType.ToolCallResult, {
      toolCallId: "tool-input-1",
      result: { content: "from-input" },
      duration: 120,
    });
    await delay(150);
    return { response: "" };
  }

  if (scenario === "queue") {
    emit(SessionEventType.TurnStarted, {}, "pty-turn-1");
    streamText("PTY_QUEUE_TURN ");
    // 保持 busy，让第二次提交进入 sendInput 队列。
    await delay(2_500);
    emit(SessionEventType.TurnComplete, { response: "PTY_QUEUE_TURN_DONE" });
    return { response: "PTY_QUEUE_TURN_DONE" };
  }

  if (scenario === "tool") {
    emit(SessionEventType.ToolCallScheduled, {
      toolCallId: "tool-1",
      toolName: "Bash",
      input: { command: "echo tool-ok" },
      schedule: { kind: "immediate" },
    });
    await delay(120);
    emit(SessionEventType.ToolCallStarted, {
      toolCallId: "tool-1",
      toolName: "Bash",
      startedAt: new Date(),
    });
    await delay(120);
    emit(SessionEventType.ToolCallResult, {
      toolCallId: "tool-1",
      result: { content: "tool-ok" },
      duration: 120,
    });
    await delay(150);
    return { response: "" };
  }

  if (scenario === "approval") {
    const result = await options.requestPermission?.({
      requestId: "pty-permission-request",
      sessionId,
      traceId,
      toolCallId: "tool-approval",
      toolName: "Bash",
      input: { command: "rm -rf /tmp/pty-fixture" },
      mode: "build",
      ruleId: "pty-rule",
      reason: "Run a destructive command in the fixture",
      riskLevel: "high",
      requestedAt: new Date(),
    });
    // 短标记：TUI 重绘会覆盖长行，`PTY_APPROVAL_allow_once` 的尾巴可能被裁掉。
    const decisionCode =
      result?.decision === "allow_once" ? "A" : result?.decision === "allow_project" ? "B" : "D";
    process.stderr.write(`PTY_APPROVAL_${decisionCode}\n`);
    recordControl(`PTY_APPROVAL_${decisionCode}`);
    return { response: "" };
  }

  // 默认：A1/B1 的文本增量场景。
  emit(SessionEventType.ModelStreaming, { kind: "start", delta: "", done: false });
  streamText("PTY_STREAM_ONE ");
  await delay(150);
  streamText("PTY_STREAM_TWO");
  await delay(150);
  emit(SessionEventType.ModelStreaming, { kind: "finish", delta: "", done: true });
  return { response: "PTY_STREAM_ONE PTY_STREAM_TWO" };
}

process.exitCode = await runTui({
  noColor: true,
  stdin: process.stdin,
  stdout: process.stdout,
  stderr: process.stderr,
  // ASCII 文案：Windows PTY 主端可能按系统代码页解码，中文会变成乱码；
  // 场景断言只依赖英文文案与 ASCII 标记。
  locale: "en-US",
  workspaceDirectory: process.cwd(),
  slashCommands: [{ name: "help", summary: "Show help", usage: "/help" }],
  submitPrompt: submit,
  // E3：Shift+Tab 的模式切换必须打到客户端能力面。
  setMode: async (mode) => {
    process.stderr.write(`PTY_MODE_${mode}\n`);
    return { mode };
  },
  // 仅 busy 时被调用；返回 queued，让 TUI 渲染排队面板。
  sendInput: async () => {
    sendInputCount += 1;
    process.stderr.write(`PTY_SEND_INPUT_${sendInputCount}\n`);
    return {
      kind: "queued" as const,
      pendingInputId: `pending-${sendInputCount}`,
      queueLength: sendInputCount,
      turnId: "pty-turn-1",
    };
  },
});
