import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

// 微信私聊默认开放（docs/specs/bot-weixin-open-dm.md）：
// UI 不得再各自手写 `privateChatMode ?? "bound_users"`，否则微信会重新显示绑定步骤。
const read = (relativePath: string): string =>
  readFileSync(new URL(relativePath, import.meta.url), "utf8");

test("私聊方式缺省解析走共享函数，微信不再被本地默认值锁成 bound_users", () => {
  for (const file of [
    "../src/BotsDialog/BotPrivateChatCard.tsx",
    "../src/BotsDialog/BotGroupChatCard.tsx",
  ]) {
    const source = read(file);
    assert.match(source, /resolveBotPrivateChatMode\(bot\)/, `${file} 必须使用共享解析函数`);
    assert.ok(
      !source.includes('bot.privateChatMode ?? "bound_users"'),
      `${file} 不得保留本地缺省值`,
    );
  }
});

test("新建 Bot 显式落盘 provider 缺省值", () => {
  const source = read("../src/BotsDialog.tsx");
  assert.match(
    source,
    /privateChatMode: resolveBotPrivateChatMode\(\{ provider: params\.provider \}\)/,
    "createDraftBot 必须显式播种解析后的缺省值",
  );
});
