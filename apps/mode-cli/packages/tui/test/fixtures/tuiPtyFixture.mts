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

type SubmitOptions = {
  onEvent?: (event: SessionEvent) => unknown;
  requestPermission?: (request: unknown, options?: unknown) => Promise<{ decision: string }>;
};

async function submit(_prompt: unknown, options: SubmitOptions) {
  submitCount += 1;
  process.stderr.write(`PTY_SUBMIT_${submitCount}\n`);
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
  submitPrompt: submit,
});
