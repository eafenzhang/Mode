import assert from "node:assert/strict";
import test from "node:test";

import { calculateOutputTps, calculatePrefillTps } from "../src/session-debug.js";

test("输出速度只接受已知生成时长", () => {
  assert.equal(calculateOutputTps(100, 2000), 50);
  assert.equal(calculateOutputTps(0, 2000), 0);
  assert.equal(calculateOutputTps(undefined, 2000), null);
  assert.equal(calculateOutputTps(100, null), null);
  assert.equal(calculateOutputTps(100, 0), null);
});

test("prefill 速度扣除缓存命中 token", () => {
  assert.equal(calculatePrefillTps(1200, 200, 1000), 1000);
  assert.equal(calculatePrefillTps(1200, 0, 1000), 1200);
  assert.equal(calculatePrefillTps(1200, undefined, 1000), 1200);
});

test("全缓存或未知 TTFT 不返回 prefill 速度", () => {
  assert.equal(calculatePrefillTps(1200, 1200, 1000), null);
  assert.equal(calculatePrefillTps(1200, 2000, 1000), null);
  assert.equal(calculatePrefillTps(1200, 200, null), null);
  assert.equal(calculatePrefillTps(undefined, 200, 1000), null);
});
