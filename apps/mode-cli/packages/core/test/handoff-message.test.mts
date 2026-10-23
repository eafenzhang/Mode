/**
 * HandoffMessage 工具（docs/specs/bot-cross-chat-handoff.md R1）：
 * 只有 Host 注入 botHandoffPort 时才能执行；输入 target/message/context/attachments
 * 与 context.toolCallId 原样交给端口，结果回执 status/target。
 * 运行前提：先 build core（dist 缺失即 import 失败）。
 */
import assert from "node:assert/strict";
import test from "node:test";

import { handoffMessageToolEntry } from "../dist/tool/handlers/handoff-message.js";
import type { ToolExecutionContext } from "../dist/tool/types.js";

function contextWith(port: ToolExecutionContext["botHandoffPort"]): ToolExecutionContext {
  return {
    toolCallId: "call-9",
    traceId: "trace-9",
    abortSignal: new AbortController().signal,
    botHandoffPort: port,
  } as unknown as ToolExecutionContext;
}

test("HandoffMessage 把输入与 toolCallId 交给 BotHandoffPort，并回执状态", async () => {
  const received: Array<Record<string, unknown>> = [];
  const context = contextWith({
    async handoff(input) {
      received.push(input);
      return { status: "submitted", target: input.target };
    },
  });
  const result = await handoffMessageToolEntry.handler(
    { target: "private:u2", message: "转达", context: "背景" },
    context,
  );
  assert.deepEqual(received, [
    { target: "private:u2", message: "转达", context: "背景", toolCallId: "call-9" },
  ]);
  assert.deepEqual(result, { status: "submitted", target: "private:u2" });
});

test("没有 BotHandoffPort 时拒绝，不把请求发到任何地方", async () => {
  const context = contextWith(undefined);
  await assert.rejects(
    () => handoffMessageToolEntry.handler({ target: "private:u2", message: "x" }, context),
    /only available in an IM bot conversation/,
  );
});
