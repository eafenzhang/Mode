import assert from "node:assert/strict";
import test from "node:test";
import { stripBotChannelContextForDisplay } from "../src/bot-channel-context.js";

// Bot 渠道上下文（docs/specs/im-bot-agent-channel-aplus.md 规则 9）：
// 注入块进入模型 prompt，但桌面/手机的用户气泡只显示真实用户输入。

test("私聊渠道上下文块只对模型可见", () => {
  const text =
    "<system-reminder>\n[渠道信息] 你正在通过企业微信私聊与用户对话。\n</system-reminder>\n\n把报告发我";
  assert.equal(stripBotChannelContextForDisplay(text), "把报告发我");
});

test("群聊上下文块、历史与引用消息行都不进用户气泡", () => {
  const group =
    "<system-reminder>\n[群聊信息] 你正在「群 A」企业微信群聊中。\n</system-reminder>\n\n" +
    "[以下是机器人未参与期间的群聊记录，仅供参考]\n" +
    "[from: 李四 2026-10-21 09:58] 前面聊了部署\n" +
    "[以上为群聊记录。接下来请回应下面的当前消息。]\n\n" +
    "[from: 张三 2026-10-21 10:00]\n[本条消息 @了你，你需要回复]\n总结一下";
  assert.equal(stripBotChannelContextForDisplay(group), "总结一下");

  const quoted = "[引用消息] 上一段对话\n\n继续解释";
  assert.equal(stripBotChannelContextForDisplay(quoted), "继续解释");
});

test("普通 system-reminder 与普通文本原样保留", () => {
  const goal = '<system-reminder source="goal-continuation">\nContinue\n</system-reminder>\n\nhi';
  assert.equal(stripBotChannelContextForDisplay(goal), goal);
  assert.equal(stripBotChannelContextForDisplay("普通消息"), "普通消息");
});

test("幂等：重复剥离结果不变", () => {
  const text = "<system-reminder>\n[Channel] private chat\n</system-reminder>\n\nhello";
  const once = stripBotChannelContextForDisplay(text);
  assert.equal(stripBotChannelContextForDisplay(once), once);
});
