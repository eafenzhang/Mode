import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";

// 纯文本通道的 markdown 降级（docs/specs/bot-reply-text-format.md）：
// 微信 iLink 只发 text_item 纯文本，业务层却是按 markdown 写的（`**/帮助**`、`` `src/a.ts` ``），
// 不降级就会把标记原样显示给用户。降级只剥离我们主动产生的标记，不能误伤 glob 与文件名。

const weixinProviderUrl = new URL("../src/bots/providers/weixinProvider.ts", import.meta.url);

test("行首与分隔符后的粗体被剥离", async () => {
  const { toPlainTextReply } = await import("../src/bots/botText.js");
  assert.equal(toPlainTextReply("**/帮助**"), "/帮助");
  assert.equal(toPlainTextReply("发送 **/帮助** 查看命令"), "发送 /帮助 查看命令");
  assert.equal(toPlainTextReply("**工具摘要**\n- read a.ts"), "工具摘要\n- read a.ts");
});

test("行内代码、标题、代码围栏、链接按纯文本呈现", async () => {
  const { toPlainTextReply } = await import("../src/bots/botText.js");
  assert.equal(toPlainTextReply("已读取 `src/a.ts`"), "已读取 src/a.ts");
  assert.equal(toPlainTextReply("#### 任务完成"), "任务完成");
  assert.equal(toPlainTextReply("```\nconst a = 1;\n```").trim(), "const a = 1;");
  assert.equal(
    toPlainTextReply("见 [文档](https://example.com/x)"),
    "见 文档 (https://example.com/x)",
  );
  // 变更摘要行的真实形态：行内代码与列表符混排。
  assert.equal(
    toPlainTextReply("- `src/a.ts` (🟢 `+3` 🔴 `-1`)"),
    "- src/a.ts (🟢 +3 🔴 -1)",
  );
});

test("不误伤 glob、文件名与列表符", async () => {
  const { toPlainTextReply } = await import("../src/bots/botText.js");
  // 单个 * 的 glob 与单下划线文件名不是强调语法，必须原样保留。
  assert.equal(toPlainTextReply("匹配 src/*.ts 与 lib/*.js"), "匹配 src/*.ts 与 lib/*.js");
  assert.equal(toPlainTextReply("my_file_name.md"), "my_file_name.md");
  assert.equal(toPlainTextReply("- 列表项一\n- 列表项二"), "- 列表项一\n- 列表项二");
  // globstar 路径的 `**` 前面是 `/`（不是强调左边界），不能被当粗体吃掉。
  assert.equal(toPlainTextReply("a/**/b/**/c"), "a/**/b/**/c");
});

test("降级幂等：已降级文本再降级不变", async () => {
  const { toPlainTextReply } = await import("../src/bots/botText.js");
  const once = toPlainTextReply("**完成**：已写入 `src/a.ts`\n\n- `+3` 行");
  assert.equal(toPlainTextReply(once), once);
  assert.ok(!once.includes("**") && !once.includes("`"));
});

test("源码契约：微信出站先降级、后 CRLF 归一化", async () => {
  const source = await readFile(weixinProviderUrl, "utf8");
  const start = source.indexOf("function buildWeixinText");
  assert.ok(start >= 0, "缺少 buildWeixinText");
  const body = source.slice(start, start + 800);
  const plainIndex = body.indexOf("toPlainTextReply(message.text)");
  assert.ok(plainIndex >= 0, "微信出站必须先做纯文本降级，否则用户会看到裸 markdown 标记");
  const crlfIndex = body.indexOf(".replace(/\\r\\n|\\r|\\n/g");
  assert.ok(crlfIndex >= 0, "CRLF 归一化不能丢");
  assert.ok(
    plainIndex < crlfIndex,
    "降级必须在 CRLF 归一化之前，顺序反了会让多行文本的边界先被改写",
  );
});
