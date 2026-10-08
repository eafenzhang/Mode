import assert from "node:assert/strict";
import test from "node:test";
import { getBotWorkspaceKey, isBotEligibleForSessionBinding } from "@mode/shared";

// 需求：会话只能绑定当前工作区的机器人。资格 = 工作区绑定表里的那个 bot，
// 或 allowedWorkspaces 显式包含本工作区（非通配）的 bot；通配 "*"（含空数组）
// 不算归属——否则任何全局 bot 都能绑进任何工作区，约束形同虚设。

const workspaceKey = "D:\\My Studio";

function bot(id: string, allowedWorkspaces: readonly string[]) {
  return { id, allowedWorkspaces };
}

test("工作区绑定的 bot 有资格（即使 allowedWorkspaces 还是通配）", () => {
  assert.equal(
    isBotEligibleForSessionBinding({
      bot: bot("bot-a", ["*"]),
      workspaceKey,
      workspaceBoundBotIds: ["bot-a"],
    }),
    true,
  );
});

test("一个工作区绑定多个 bot：列表里的每个 bot 都有资格", () => {
  assert.equal(
    isBotEligibleForSessionBinding({
      bot: bot("bot-b", []),
      workspaceKey,
      workspaceBoundBotIds: ["bot-a", "bot-b"],
    }),
    true,
  );
  assert.equal(
    isBotEligibleForSessionBinding({
      bot: bot("bot-c", []),
      workspaceKey,
      workspaceBoundBotIds: ["bot-a", "bot-b"],
    }),
    false,
  );
});

test("显式授权本工作区的 bot 有资格", () => {
  assert.equal(
    isBotEligibleForSessionBinding({
      bot: bot("bot-a", ["D:\\Other", workspaceKey]),
      workspaceKey,
      workspaceBoundBotIds: [],
    }),
    true,
  );
});

test("通配范围（含空数组）不算归属当前工作区", () => {
  for (const allowed of [["*"], []] as const) {
    assert.equal(
      isBotEligibleForSessionBinding({
        bot: bot("bot-global", allowed),
        workspaceKey,
        workspaceBoundBotIds: [],
      }),
      false,
    );
  }
});

test("只授权了其他工作区的 bot 没有资格", () => {
  assert.equal(
    isBotEligibleForSessionBinding({
      bot: bot("bot-b", ["D:\\Other Studio"]),
      workspaceKey,
      workspaceBoundBotIds: ["bot-c"],
    }),
    false,
  );
});

test("当前工作区绑定的是别的 bot 时，本 bot 也不会因绑定而获得资格", () => {
  assert.equal(
    isBotEligibleForSessionBinding({
      bot: bot("bot-b", []),
      workspaceKey,
      workspaceBoundBotIds: ["bot-c"],
    }),
    false,
  );
});

test("空白条目不算归属：空串/纯空白 allowedWorkspaces 与空 key 都判无资格", () => {
  assert.equal(
    isBotEligibleForSessionBinding({
      bot: bot("bot-a", ["", "  "]),
      workspaceKey,
      workspaceBoundBotIds: [],
    }),
    false,
  );
  assert.equal(
    isBotEligibleForSessionBinding({
      bot: bot("bot-a", [workspaceKey]),
      workspaceKey: "   ",
      workspaceBoundBotIds: [],
    }),
    false,
  );
});

test("getBotWorkspaceKey：workspaceIdentity 优先，与 service 端 key 规则一致", () => {
  assert.equal(getBotWorkspaceKey("D:\\My Studio"), "D:\\My Studio");
  assert.equal(getBotWorkspaceKey("D:\\My Studio", "  ident-1  "), "ident-1");
  // 空白身份退回路径，避免把 "  " 当成一个工作区身份
  assert.equal(getBotWorkspaceKey("D:\\My Studio", "   "), "D:\\My Studio");
  assert.equal(getBotWorkspaceKey("D:\\My Studio", ""), "D:\\My Studio");
});
