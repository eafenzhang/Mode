/**
 * SendFile 协议端口（docs/specs/bot-outbound-media.md）：
 * 目标只能来自本会话的 activeBotDeliveryTarget，模型参数不得覆盖；
 * 端口只把 path/caption 送到 Host，路径校验与目标解析留在 botsService。
 * 运行：pnpm --filter @mode/bootstrap test
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { createProtocolBotMediaPort } from "../src/mode-protocol/bot-media-port.js";
import type {
  ModeProtocolAgentServerContext,
  ModeProtocolSessionRecord,
} from "../src/mode-protocol/server-types.js";

function recordWithTarget(
  target: ModeProtocolSessionRecord["activeBotDeliveryTarget"],
): ModeProtocolSessionRecord {
  return { activeBotDeliveryTarget: target } as unknown as ModeProtocolSessionRecord;
}

function contextWithRequestClient(calls: Array<{ method: string; params: unknown }>) {
  return {
    requestClient: async (method: string, params: unknown) => {
      calls.push({ method, params });
      return { filename: "report.pdf", kind: "file" };
    },
  } as unknown as ModeProtocolAgentServerContext;
}

test("SendFile 端口注入 bot 目标，并原样透传 path/caption", async () => {
  const calls: Array<{ method: string; params: unknown }> = [];
  const target = {
    provider: "wecom" as const,
    botId: "bot-1",
    providerUserId: "chat-1",
    chatType: "group" as const,
  };
  const port = createProtocolBotMediaPort(contextWithRequestClient(calls), () =>
    recordWithTarget(target),
  );

  const result = await port.sendMedia({
    filePath: "reports/today.md",
    caption: "今天的报告",
  });
  assert.deepEqual(result, { filename: "report.pdf", kind: "file" });
  assert.equal(calls.length, 1);
  assert.equal(calls[0]!.method, "bots/sendMedia");
  assert.deepEqual(calls[0]!.params, {
    target,
    filePath: "reports/today.md",
    caption: "今天的报告",
  });
});

test("SendFile 没有 bot 目标时拒绝，不发起任何 Host 请求", async () => {
  const calls: Array<{ method: string; params: unknown }> = [];
  const port = createProtocolBotMediaPort(contextWithRequestClient(calls), () =>
    recordWithTarget(undefined),
  );

  await assert.rejects(
    () => port.sendMedia({ filePath: "reports/today.md" }),
    /No active IM bot delivery target/,
  );
  assert.equal(calls.length, 0, "缺少目标时不能向 Host 发请求");
});
