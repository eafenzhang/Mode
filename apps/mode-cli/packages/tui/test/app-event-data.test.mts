import assert from "node:assert/strict";
import test from "node:test";

import { contextWindowFromPayload, formatEventError } from "../src/app-event-data.js";

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
