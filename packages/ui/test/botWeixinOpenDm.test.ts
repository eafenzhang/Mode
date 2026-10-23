import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

// 微信私聊默认开放（docs/specs/bot-weixin-open-dm.md）：
// UI 不得再各自手写 `privateChatMode ?? "bound_users"`，否则微信会重新显示绑定步骤。
// 2026-10-23 修订：缺省统一 whitelist，见 docs/specs/bot-access-mode.md R5。
const read = (relativePath: string): string =>
  readFileSync(new URL(relativePath, import.meta.url), "utf8");

test("访问模式缺省解析走共享函数，微信不被本地默认值锁死", () => {
  for (const file of [
    "../src/BotsDialog/BotPrivateChatCard.tsx",
    "../src/BotsDialog/BotGroupChatCard.tsx",
  ]) {
    const source = read(file);
    assert.match(source, /resolveBotAccessMode\(bot\)/, `${file} 必须使用共享解析函数`);
    assert.ok(
      !source.includes('bot.privateChatMode ?? "bound_users"'),
      `${file} 不得保留本地缺省值`,
    );
  }
});

test("新建 Bot 显式落盘 accessMode 缺省值", () => {
  const source = read("../src/BotsDialog.tsx");
  assert.match(
    source,
    /accessMode:\s*"whitelist"/,
    "createDraftBot 必须显式播种 accessMode whitelist",
  );
});
