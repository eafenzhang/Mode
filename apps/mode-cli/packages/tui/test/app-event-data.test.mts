import assert from "node:assert/strict";
import test from "node:test";

import {
  cacheStatsFromPayload,
  contextWindowFromPayload,
  formatEventError,
  usageFromPayload,
} from "../src/app-event-data.js";

test("contextWindowFromPayload：只认非负数值的 contextWindow", () => {
  assert.equal(contextWindowFromPayload({ contextWindow: 1000 }), 1000);
  assert.equal(contextWindowFromPayload({ contextWindow: 0 }), 0);
  assert.equal(contextWindowFromPayload({ contextWindow: 1.5 }), 1.5);
  assert.equal(contextWindowFromPayload({ contextWindow: -1 }), undefined);
  assert.equal(contextWindowFromPayload({ context_window: 2000 }), undefined);
  assert.equal(contextWindowFromPayload({ contextWindow: "3000" }), undefined);
  assert.equal(contextWindowFromPayload({}), undefined);
});

test("formatEventError：message/嵌套 error.message，其余回退 Unknown error", () => {
  assert.equal(formatEventError({ message: "hi" }), "hi");
  assert.equal(formatEventError({ error: { message: "nested" } }), "nested");
  assert.equal(formatEventError({ error: "boom" }), "Unknown error");
  assert.equal(formatEventError({ message: 42 }), "Unknown error");
  assert.equal(formatEventError({}), "Unknown error");
});

test("usageFromPayload：provider 计数形态补齐默认 0", () => {
  assert.deepEqual(
    usageFromPayload({ usage: { modelRequestCount: 2, inputTokens: 10, outputTokens: 5 } }),
    {
      cacheReadTokens: 0,
      cacheWriteTokens: 0,
      inputTokens: 10,
      modelRequestCount: 2,
      outputTokens: 5,
      reasoningTokens: 0,
      source: "provider",
      totalTokens: 0,
      webFetchRequests: 0,
      webSearchRequests: 0,
    },
  );
});

test("usageFromPayload：serverToolUse 形态推导请求数与总量", () => {
  const usage = usageFromPayload({
    usage: {
      inputTokens: 7,
      outputTokens: 3,
      serverToolUse: { webSearchRequests: 1 },
    },
  });
  assert.equal(usage?.modelRequestCount, 1);
  assert.equal(usage?.totalTokens, 10);
  assert.equal(usage?.webSearchRequests, 1);
  assert.equal(usage?.webFetchRequests, 0);
});

test("usage/cache 提取：空对象或缺失字段返回 undefined", () => {
  assert.equal(usageFromPayload({ usage: {} }), undefined);
  assert.equal(usageFromPayload({}), undefined);
  assert.equal(cacheStatsFromPayload({ cacheStats: {} }), undefined);
  assert.equal(cacheStatsFromPayload({}), undefined);
});

test("cacheStatsFromPayload：保留部分字段", () => {
  const stats = cacheStatsFromPayload({ cacheStats: { cachedMessages: 3, lastCacheHit: true } });
  assert.equal(stats?.cachedMessages, 3);
  assert.equal(stats?.lastCacheHit, true);
  assert.equal(stats?.cacheReadTokens, undefined);
  assert.equal(stats?.totalMessages, undefined);
});
