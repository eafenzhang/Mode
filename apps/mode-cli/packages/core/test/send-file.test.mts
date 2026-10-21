/**
 * SendFile 工具（docs/specs/bot-outbound-media.md）：
 * 只有 Host 注入 botMediaPort 时才能执行；输入 path/caption 原样交给端口，
 * 结果回执带文件名与类型。
 * 运行前提：先 build core（dist 缺失即 import 失败）。
 */
import assert from "node:assert/strict";
import test from "node:test";

import { sendFileToolEntry } from "../dist/tool/handlers/send-file.js";
import type { ToolExecutionContext } from "../dist/tool/types.js";

function contextWith(port: ToolExecutionContext["botMediaPort"]): ToolExecutionContext {
  return {
    toolCallId: "call-1",
    traceId: "trace-1",
    abortSignal: new AbortController().signal,
    botMediaPort: port,
  } as unknown as ToolExecutionContext;
}

test("SendFile 把 path/caption 交给 BotMediaPort，并回执 sent", async () => {
  const received: Array<{ filePath: string; caption?: string }> = [];
  const context = contextWith({
    async sendMedia(input) {
      received.push(input);
      return { filename: "report.pdf", kind: "file" };
    },
  });

  const result = await sendFileToolEntry.handler(
    { path: "reports/today.pdf", caption: "今天的报告" },
    context,
  );

  assert.deepEqual(received, [{ filePath: "reports/today.pdf", caption: "今天的报告" }]);
  assert.deepEqual(result, { status: "sent", filename: "report.pdf", kind: "file" });
});

test("没有 BotMediaPort 时拒绝，不把请求发到任何地方", async () => {
  const context = contextWith(undefined);
  await assert.rejects(
    () => sendFileToolEntry.handler({ path: "reports/today.pdf" }, context),
    /only available in an IM bot conversation/,
  );
});
