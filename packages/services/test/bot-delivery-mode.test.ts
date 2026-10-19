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
