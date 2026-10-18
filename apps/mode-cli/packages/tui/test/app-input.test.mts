import assert from "node:assert/strict";
import test from "node:test";

import { clampIndex, modelCommandQuery } from "../src/app-input.js";

test("clampIndex：负数归 0，越界归末位，空集合归 0", () => {
  assert.equal(clampIndex(-1, 3), 0);
  assert.equal(clampIndex(0, 3), 0);
  assert.equal(clampIndex(2, 3), 2);
  assert.equal(clampIndex(5, 3), 2);
  assert.equal(clampIndex(1, 0), 0);
});

test("modelCommandQuery：/model 前缀解析，仅空格分隔的查询保留原样", () => {
  assert.equal(modelCommandQuery("/model"), "");
  assert.equal(modelCommandQuery("/model "), "");
  assert.equal(modelCommandQuery("/model gpt"), "gpt");
  assert.equal(modelCommandQuery("/model   gpt"), "  gpt");
  assert.equal(modelCommandQuery("hello"), undefined);
  assert.equal(modelCommandQuery(" /model gpt"), undefined);
  assert.equal(modelCommandQuery("/MODEL gpt"), undefined);
  assert.equal(modelCommandQuery("/model\tgpt"), undefined);
});
