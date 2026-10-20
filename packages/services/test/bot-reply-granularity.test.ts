import assert from "node:assert/strict";
import test from "node:test";
import { botsConfigFileSchema, normalizeBotReplyGranularity } from "@mode/shared";

// 回复粒度默认值（docs/specs/bot-reply-granularity.md）：
//   1. 默认按 provider 区分——企微未设置时 streaming_card（对齐 MyAgents 打字机观感），
//      其余 provider 保持 assistant_changes；
//   2. 显式写入的 replyMode 一律优先于默认；读取期不得再把企微 assistant_changes
//      翻回 streaming_card（2026-10-20 修复：否则设置页无法切换标准模式）。

function botFixture(provider: string, replyMode: string): Record<string, unknown> {
  return {
    id: `bot-${provider}-1`,
    name: "fixture",
    provider,
    enabled: true,
    allowedWorkspaces: ["*"],
    allowedCommands: {
      status: true,
      new: true,
      workspace: true,
      model: true,
      mode: true,
      thoughtLevel: true,
      reply: true,
    },
    currentOptions: {},
    replyMode,
  };
}

test("provider 感知默认：企微未设置为 streaming_card，其余保持 assistant_changes", () => {
  assert.equal(normalizeBotReplyGranularity("wecom", undefined), "streaming_card");
  assert.equal(normalizeBotReplyGranularity("telegram", undefined), "assistant_changes");
  assert.equal(normalizeBotReplyGranularity("dingtalk", undefined), "assistant_changes");
  assert.equal(normalizeBotReplyGranularity("weixin", undefined), "assistant_changes");
  // 飞书系只支持一档：无论默认是什么，解析结果恒为 streaming_card。
  assert.equal(normalizeBotReplyGranularity("lark", undefined), "streaming_card");
  assert.equal(normalizeBotReplyGranularity("feishu", undefined), "streaming_card");
});

test("显式 replyMode 优先于 provider 默认（含企微显式 assistant_changes）", () => {
  assert.equal(normalizeBotReplyGranularity("wecom", "assistant_changes"), "assistant_changes");
  assert.equal(normalizeBotReplyGranularity("wecom", "summary_changes"), "summary_changes");
  assert.equal(
    normalizeBotReplyGranularity("wecom", "assistant_toolcalls_changes"),
    "assistant_toolcalls_changes",
  );
  // 不受支持的显式值仍回落到该 provider 的首档支持项（微信无流式通道）。
  assert.equal(normalizeBotReplyGranularity("weixin", "streaming_card"), "assistant_changes");
});

test("显式 replyMode 读取后保持原值（企微标准回复可切换）", () => {
  const parse = (bots: Record<string, unknown>[]): unknown =>
    botsConfigFileSchema.parse({ version: 3, bots }).bots;

  const [wecomStandard] = parse([botFixture("wecom", "assistant_changes")]) as [
    Record<string, unknown>,
  ];
  assert.equal(
    wecomStandard.replyMode,
    "assistant_changes",
    "企微显式选「标准回复」不能被读取期迁移翻回 streaming_card",
  );

  const [wecomStream] = parse([botFixture("wecom", "streaming_card")]) as [Record<string, unknown>];
  assert.equal(wecomStream.replyMode, "streaming_card");

  const [wecomSummary] = parse([botFixture("wecom", "summary_changes")]) as [
    Record<string, unknown>,
  ];
  assert.equal(wecomSummary.replyMode, "summary_changes");

  const [dingtalk] = parse([botFixture("dingtalk", "assistant_changes")]) as [
    Record<string, unknown>,
  ];
  assert.equal(dingtalk.replyMode, "assistant_changes");
});
