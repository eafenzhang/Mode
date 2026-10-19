import assert from "node:assert/strict";
import test from "node:test";

import {
  asRecord,
  booleanField,
  describeSessionEvent,
  formatDuration,
  formatNumber,
  matchesText,
  normalizePromptInput,
  numberField,
  stringField,
  truncatePlain,
} from "../src/state.js";

test("字段读取：asRecord 只接受对象，string/number 字段做类型校验", () => {
  assert.deepEqual(asRecord({ a: 1 }), { a: 1 });
  assert.deepEqual(asRecord(null), {});
  assert.deepEqual(asRecord("x"), {});
  assert.deepEqual(asRecord([1, 2]), [1, 2]);

  assert.equal(stringField({ value: "  x  " }, "value"), "  x  ");
  assert.equal(stringField({ value: "   " }, "value"), "   ");
  assert.equal(stringField({ value: 1 }, "value"), undefined);
  assert.equal(numberField({ value: 1.5 }, "value"), 1.5);
  assert.equal(numberField({ value: Number.NaN }, "value"), undefined);
  assert.equal(numberField({ value: Number.POSITIVE_INFINITY }, "value"), undefined);
  assert.equal(booleanField({ value: true }, "value"), true);
  assert.equal(booleanField({ value: "true" }, "value"), undefined);
});

test("格式化：数字千分位与耗时单位", () => {
  assert.equal(formatNumber(1234567), "1,234,567");
  assert.equal(formatDuration(undefined), "-");
  assert.equal(formatDuration(0), "0ms");
  assert.equal(formatDuration(1500), "1.5s");
  assert.equal(formatDuration(60000), "60s");
});

test("文本截断：超长时连省略号一起计入上限", () => {
  assert.equal(truncatePlain("hello", 10), "hello");
  assert.equal(truncatePlain("hello", 5), "hello");
  assert.equal(truncatePlain("hello world", 8), "hello...");
});

test("文本匹配：大小写不敏感包含，空查询恒匹配", () => {
  assert.equal(matchesText("ALP", ["alpha", "beta"]), true);
  assert.equal(matchesText("zzz", ["alpha"]), false);
  assert.equal(matchesText("", ["alpha"]), true);
  assert.equal(matchesText("x", [undefined, "x"]), true);
});

test("prompt 归一：字符串零附件，对象取 text 与附件数", () => {
  assert.deepEqual(normalizePromptInput("  hi  "), { attachmentCount: 0, text: "  hi  " });
  assert.deepEqual(
    normalizePromptInput({
      text: "hi",
      attachments: [{ type: "file", path: "/a" }],
    } as never),
    { attachmentCount: 1, text: "hi" },
  );
});

test("事件描述：下划线转空格，非法输入回退 session event", () => {
  assert.equal(describeSessionEvent({ type: "turn_started" } as never), "turn started");
  assert.equal(describeSessionEvent(null as never), "session event");
  assert.equal(describeSessionEvent({} as never), "session event");
  assert.equal(describeSessionEvent({ type: 42 } as never), "session event");
});
