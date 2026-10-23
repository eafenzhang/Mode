import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import enUS from "../src/i18n/locales/en-US";
import faIR from "../src/i18n/locales/fa";

// 访问控制统一双模式（docs/specs/bot-access-mode.md R5/R6）：
// 新建渠道显式落盘 whitelist；模式与黑名单由统一入口解析/操作，UI 不手写缺省。

const read = (relativePath: string): string =>
  readFileSync(new URL(relativePath, import.meta.url), "utf8");

test("createDraftBot 显式落盘 accessMode whitelist（全渠道，含微信）", () => {
  const source = read("../src/BotsDialog.tsx");
  assert.match(source, /accessMode:\s*"whitelist"/, "createDraftBot 必须写 accessMode");
  assert.ok(
    !source.includes("privateChatMode: resolveBotPrivateChatMode"),
    "不得再写 privateChatMode 缺省",
  );
});

test("访问模式卡按 resolveBotAccessMode 取值，操作全部经 props 上抛", () => {
  const source = read("../src/BotsDialog/BotAccessModeCard.tsx");
  assert.match(source, /resolveBotAccessMode\(bot\)/);
  assert.match(source, /onSetAccessMode/);
  assert.match(source, /onUnblockUser/);
  assert.match(source, /onUnblockGroup/);
  // 卡片不直接依赖服务实例：服务方法接线在 BotsDialog.tsx
  const dialog = read("../src/BotsDialog.tsx");
  assert.match(dialog, /setBotAccessMode/);
  assert.match(dialog, /unblockBotUser/);
  assert.match(dialog, /unblockBotGroup/);
});

test("私聊卡按 accessMode 分支：free 展示 userRegistry，whitelist 保留绑定码", () => {
  const source = read("../src/BotsDialog/BotPrivateChatCard.tsx");
  assert.match(source, /resolveBotAccessMode\(bot\)/);
  assert.match(source, /userRegistry/);
  assert.ok(!source.includes('privateChatMode ?? "bound_users"'), "不得保留本地缺省值");
});

test("群卡 free 模式无审批按钮", () => {
  const source = read("../src/BotsDialog/BotGroupChatCard.tsx");
  assert.match(source, /resolveBotAccessMode\(bot\)/);
  assert.match(source, /accessMode === "free"/, "free 分支必须显式存在");
});

test("fa 的访问模式与登记列表文案已完成波斯文翻译（不得回退成英文占位）", () => {
  const keys = [
    "bots.groupChat.registered",
    "bots.groupChat.registeredEmpty",
    "bots.groupChat.block",
    "bots.accessMode.title",
    "bots.accessMode.description",
    "bots.accessMode.mode.whitelist",
    "bots.accessMode.mode.free",
    "bots.accessMode.blockedUsers",
    "bots.accessMode.blockedGroups",
    "bots.accessMode.unblockUser",
    "bots.accessMode.unblockGroup",
    "bots.accessMode.empty",
    "bots.registry.title",
    "bots.registry.empty",
    "bots.registry.remove",
    "bots.registry.block",
    "bots.registry.blockedBadge",
  ] as const;
  for (const key of keys) {
    const fa = faIR[key];
    assert.ok(fa, `fa 缺少 ${key}`);
    assert.ok(/[\u0600-\u06FF]/.test(fa), `${key} 必须是波斯文，当前为：${fa}`);
    assert.notEqual(fa, enUS[key], `${key} 不得与英文占位相同`);
  }
});
