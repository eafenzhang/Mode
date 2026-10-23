/**
 * HandoffMessage 协议端口（docs/specs/bot-cross-chat-handoff.md R1/R3）：
 * 源 bot 身份只从本会话的 activeBotDeliveryTarget 读取（模型无法冒充别的 bot），
 * 端口只把 conversationKey/message/context/attachments/toolCallId 送到 Host，
 * 目标归属校验、黑名单与准入全部留在 botsService.handoffMessage。
 * 镜像 bot-media-port.test.mts 的两条用例（透传 + 无目标拒绝）。
 * 运行：pnpm --filter @mode/bootstrap test
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { createProtocolBotHandoffPort } from "../src/mode-protocol/bot-handoff-port.js";
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
      // Host 侧回执 schema：modeBotHandoffResultSchema（status + conversationKey）。
      const conversationKey = (params as { conversationKey?: string }).conversationKey;
      return { status: "submitted", conversationKey };
    },
  } as unknown as ModeProtocolAgentServerContext;
}

test("HandoffMessage 端口注入本会话 bot 目标，并原样透传交接参数", async () => {
  const calls: Array<{ method: string; params: unknown }> = [];
  const target = {
    provider: "wecom" as const,
    botId: "bot-1",
    providerUserId: "u1",
    chatType: "private" as const,
  };
  const port = createProtocolBotHandoffPort(contextWithRequestClient(calls), () =>
    recordWithTarget(target),
  );

  const result = await port.handoff({
    target: "group:g1",
    message: "请转达",
    context: "背景说明",
    toolCallId: "call-7",
  });
  assert.deepEqual(result, { status: "submitted", target: "group:g1" });
  assert.equal(calls.length, 1);
  assert.equal(calls[0]!.method, "bots/handoff");
  assert.deepEqual(calls[0]!.params, {
    target,
    conversationKey: "group:g1",
    message: "请转达",
    context: "背景说明",
    toolCallId: "call-7",
  });
});

test("HandoffMessage 端口无 bot 目标时拒绝，不发起任何 Host 请求", async () => {
  const calls: Array<{ method: string; params: unknown }> = [];
  const port = createProtocolBotHandoffPort(contextWithRequestClient(calls), () =>
    recordWithTarget(undefined),
  );

  await assert.rejects(
    () => port.handoff({ target: "private:u2", message: "x", toolCallId: "call-8" }),
    /No active IM bot delivery target/,
  );
  assert.equal(calls.length, 0, "缺少目标时不能向 Host 发请求");
});
