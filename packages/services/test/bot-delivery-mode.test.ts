import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { botsConfigFileSchema } from "@mode/shared";

// A+ 投递模式：auto / tool_only 必须进入严格 schema，并在出站路径真正生效。
// 命令回执、权限/提问属于控制消息，不受 tool_only 影响。

test("botConfigSchema：deliveryMode 与 groupChat.permissions 可往返", () => {
  const parsed = botsConfigFileSchema.parse({
    version: 3,
    bots: [
      {
        id: "bot-1",
        name: "bot",
        provider: "wecom",
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
        replyMode: "streaming_card",
        deliveryMode: "tool_only",
        groupChat: {
          activation: "mention",
          permissions: [
            {
              groupId: "group-1",
              groupName: "Team",
              platform: "wecom",
              status: "approved",
              discoveredAt: 1,
            },
          ],
        },
      },
    ],
  });
  const bot = parsed.bots[0]!;
  assert.equal(bot.deliveryMode, "tool_only");
  assert.equal(bot.groupChat?.permissions?.[0]?.status, "approved");
});

test("源码契约：tool_only 只抑制 automatic 输出，并跳过流式卡片", async () => {
  const source = await readFile(new URL("../src/bots/botsService.ts", import.meta.url), "utf8");
  assert.ok(
    source.includes('bot.deliveryMode === "tool_only" && options.automatic === true'),
    "sendOutbound 缺少 automatic 抑制",
  );
  const cardIndex = source.indexOf("const syncStreamingCardReply");
  assert.ok(cardIndex >= 0, "缺少 syncStreamingCardReply");
  const cardBody = source.slice(cardIndex, cardIndex + 500);
  assert.ok(cardBody.includes('bot.deliveryMode === "tool_only"'), "流式卡片缺少 tool_only 早退");
});

test("源码契约：普通 AI 输出全部标记 automatic，权限回复保持控制消息", async () => {
  const source = await readFile(new URL("../src/bots/botsService.ts", import.meta.url), "utf8");

  const flushIndex = source.indexOf("const flushAssistantReplyBuffer = async");
  assert.ok(flushIndex >= 0, "缺少正文刷出函数");
  const flushEnd = source.indexOf(
    "const modeTaskService = await resolveModeTaskServiceForContext",
    flushIndex,
  );
  assert.ok(flushEnd > flushIndex, "无法界定正文刷出函数边界");
  assert.ok(
    source.slice(flushIndex, flushEnd).includes("{ automatic: true }"),
    "正文逐块刷出必须标记 automatic，否则 tool_only 会漏发",
  );

  const finalLoopIndex = source.indexOf("for (const text of replyMessages)");
  assert.ok(finalLoopIndex >= 0, "缺少终稿发送循环");
  assert.ok(
    source.slice(finalLoopIndex, finalLoopIndex + 400).includes("{ automatic: true }"),
    "终稿必须标记 automatic，否则 tool_only 会漏发",
  );

  const toolIndex = source.indexOf("formatBotToolCallReply(toolCall");
  assert.ok(toolIndex >= 0, "缺少工具摘要发送");
  assert.ok(
    source.slice(toolIndex, toolIndex + 600).includes("{ automatic: true }"),
    "工具摘要属于普通 AI 输出，必须标记 automatic",
  );

  // 权限卡片/选项是控制消息：tool_only 不能把它吞掉，否则用户无法响应权限请求。
  const permissionIndex = source.indexOf("sendOutbound(bot, permissionReply)");
  assert.ok(permissionIndex >= 0, "权限回复发送路径缺失");
  assert.ok(
    !source.slice(permissionIndex, permissionIndex + 160).includes("automatic: true"),
    "权限回复是控制消息，不能标记 automatic",
  );
});
