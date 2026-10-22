/**
 * PTY harness 的 TUI 夹具：真 `runTui` + 假运行时。
 *
 * 只注入 `submitPrompt`（按固定脚本发 `model_streaming` 文本增量）与最小启动参数；
 * 不接模型、不读凭据、不写数据根。PTY 由测试侧 `node-pty` 提供（stdin/stdout 都是 TTY）。
 *
 * 输出标记（供测试断言）：
 * - `PTY_STREAM_<chunk>`：流式文本增量渲染后的可见文本；
 * - `PTY_SUBMIT_<n>`：`submitPrompt` 收到的第 n 次提交（写到 stdout 会被 TUI 覆盖，
 *   所以这里改写到 stderr，测试同时收集 stderr）。
 */
import { randomUUID } from "node:crypto";

import { SessionEventType, type SessionEvent } from "@mode/contracts";
import { runTui } from "../../src/index.js";

const sessionId = "pty-fixture-session";
let sequence = 0;
let submitCount = 0;

function event(type: SessionEvent["type"], payload: unknown): SessionEvent {
  sequence += 1;
  return {
    id: randomUUID(),
    sessionId,
    timestamp: new Date(),
    traceId: "pty-fixture-trace",
    sequenceNumber: sequence,
    type,
    payload,
  };
}

const delay = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

const fixture = {
  async submitPrompt(_prompt: unknown, options: { onEvent?: (event: SessionEvent) => unknown }) {
    submitCount += 1;
    process.stderr.write(`PTY_SUBMIT_${submitCount}\n`);
    const emit = (payload: Record<string, unknown>) => {
      void options.onEvent?.(event(SessionEventType.ModelStreaming, payload));
    };
    emit({ kind: "start", delta: "", done: false });
    emit({ kind: "text_delta", delta: "PTY_STREAM_ONE ", done: false });
    await delay(150);
    emit({ kind: "text_delta", delta: "PTY_STREAM_TWO", done: false });
    await delay(150);
    emit({ kind: "finish", delta: "", done: true });
    return { response: "PTY_STREAM_ONE PTY_STREAM_TWO" };
  },
};

process.exitCode = await runTui({
  noColor: true,
  stdin: process.stdin,
  stdout: process.stdout,
  stderr: process.stderr,
  // ASCII 文案：Windows PTY 主端可能按系统代码页解码，中文会变成乱码；
  // 场景断言只依赖英文文案与 ASCII 标记。
  locale: "en-US",
  workspaceDirectory: process.cwd(),
  submitPrompt: fixture.submitPrompt,
});
