import assert from "node:assert/strict";
import test from "node:test";

import { applyTurnCompleteFallbackResponse } from "../src/app-turn-complete.js";

function run(payload: Record<string, unknown>, previous: unknown[] = []) {
  let calls = 0;
  let next: unknown = previous;
  applyTurnCompleteFallbackResponse(
    payload as never,
    ((value: unknown) => {
      calls += 1;
      next =
        typeof value === "function" ? (value as (current: unknown[]) => unknown)(previous) : value;
    }) as never,
  );
  return { calls, next };
}

test("turn complete 回退：非空 response 追加投影 agent 消息", () => {
  assert.deepEqual(run({ response: "hi" }), {
    calls: 1,
    next: [{ content: "hi", role: "agent", streamProjected: true }],
  });
  assert.deepEqual(run({ response: "hi" }, [{ role: "user", content: "q" }]), {
    calls: 1,
    next: [
      { role: "user", content: "q" },
      { content: "hi", role: "agent", streamProjected: true },
    ],
  });
});

test("turn complete 回退：空、空白或缺失 response 不触发写入", () => {
  assert.deepEqual(run({ response: "" }), { calls: 0, next: [] });
  assert.deepEqual(run({ response: "  " }), { calls: 0, next: [] });
  assert.deepEqual(run({}), { calls: 0, next: [] });
});

test("turn complete 回退：messageId 不进入投影消息", () => {
  assert.deepEqual(run({ response: "hi", messageId: "m1" }), {
    calls: 1,
    next: [{ content: "hi", role: "agent", streamProjected: true }],
  });
});
