import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";


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

