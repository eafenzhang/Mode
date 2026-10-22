import assert from "node:assert/strict";
import test from "node:test";
import type { BotConfig, BotRuntimeInfo } from "@mode/shared";
import { projectBotRuntimeInfo } from "../src/bots/botRuntimeProjection.js";

const weixinBot: Pick<BotConfig, "id" | "provider" | "enabled"> = {
  id: "bot-weixin",
  provider: "weixin",
  enabled: true,
};

const pollingRuntime: BotRuntimeInfo = {
  botId: "bot-weixin",
  provider: "weixin",
  status: "polling",
  message: "微信长轮询运行中",
};

// 回归：微信 Bot 启用后一定存在 runtime 条目；激活标记只在 fallback 分支返回时，
// 首条入站消息写入 weixinActivatedAt 后 UI 仍显示「待激活」。
test("已有 runtime 时仍合并通道级 weixinActivatedAt", () => {
  const result = projectBotRuntimeInfo({
    bot: weixinBot,
    runtime: pollingRuntime,
    channelState: { weixinActivatedAt: 1792003155279 },
  });
  assert.equal(result.status, "polling");
  assert.equal(result.weixinActivatedAt, 1792003155279);
});

test("未激活时不伪造 weixinActivatedAt 字段", () => {
  const result = projectBotRuntimeInfo({
    bot: weixinBot,
    runtime: pollingRuntime,
    channelState: { weixinActivatedAt: undefined },
  });
  assert.equal("weixinActivatedAt" in result, false);
});

test("没有 runtime 时保留 idle/disabled 兜底，并同样带激活标记", () => {
  assert.deepEqual(
    projectBotRuntimeInfo({ bot: weixinBot, runtime: undefined, channelState: undefined }),
    {
      botId: "bot-weixin",
      provider: "weixin",
      status: "idle",
      message: "Bot is configured.",
    },
  );
  const disabled = projectBotRuntimeInfo({
    bot: { ...weixinBot, enabled: false },
    runtime: undefined,
    channelState: { weixinActivatedAt: 1 },
  });
  assert.equal(disabled.status, "disabled");
  assert.equal(disabled.weixinActivatedAt, 1);
});
