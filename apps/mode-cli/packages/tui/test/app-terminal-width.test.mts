import assert from "node:assert/strict";
import test from "node:test";

import { displayWidth, truncateDisplay, wordWrappedLineCount } from "../src/app-terminal-width.js";

// 黑盒契约：只验证公开导出的行为，不读取实现。
test("displayWidth：ASCII 与东亚宽字符", () => {
  assert.equal(displayWidth("abc"), 3);
  assert.equal(displayWidth("你好"), 4);
  assert.equal(displayWidth("a你"), 3);
  assert.equal(displayWidth(""), 0);
});

test("truncateDisplay：超宽加省略号，未超宽原样返回", () => {
  assert.equal(truncateDisplay("abcdef", 4), "a...");
  assert.equal(truncateDisplay("abc", 4), "abc");
  assert.equal(truncateDisplay("你好世界", 4), "...");
  assert.equal(truncateDisplay("", 3), "");
});

test("wordWrappedLineCount：空串至少一行，超宽按宽度折行", () => {
  assert.equal(wordWrappedLineCount("", 3), 1);
  assert.equal(wordWrappedLineCount("a b c", 3), 2);
  assert.equal(wordWrappedLineCount("abcdef", 3), 2);
});
