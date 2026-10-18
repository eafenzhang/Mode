import assert from "node:assert/strict";
import test from "node:test";

import { compactCommandFromText } from "../src/app-compact-timeline.js";

test("compact 命令识别：前缀 /compact，大小写归一、空白折叠", () => {
  assert.equal(compactCommandFromText("/compact"), "/compact");
  assert.equal(compactCommandFromText("/compact now"), "/compact now");
  assert.equal(compactCommandFromText("  /compact  "), "/compact");
  assert.equal(compactCommandFromText("/Compact"), "/compact");
  assert.equal(compactCommandFromText("/compact\nsecond"), "/compact second");
});

test("compact 命令识别：非开头或无词边界的输入不认", () => {
  assert.equal(compactCommandFromText("hello /compact"), undefined);
  assert.equal(compactCommandFromText("/compactx"), undefined);
  assert.equal(compactCommandFromText("compact"), undefined);
  assert.equal(compactCommandFromText(""), undefined);
});
