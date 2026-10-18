/**
 * M4 adapter：toast 队列与栈定位语义。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import {
  getToastItems,
  patchToast,
  pushToast,
  removeToast,
  resetToasts,
  resolveToastStackClassName,
  subscribeToasts,
  upsertToastItem,
  type ToastItem,
} from "../src/ui-foundation/adapters/toastStore.js";

function item(id: number, dedupeKey?: string): ToastItem {
  return { id, message: `m${id}`, durationMs: 3000, position: "bottom-center", dedupeKey };
}

test("upsertToastItem：无 dedupeKey 追加，有则替换并移到末尾", () => {
  assert.deepEqual(
    upsertToastItem([item(1), item(2)], item(3)).map((entry) => entry.id),
    [1, 2, 3],
  );
  assert.deepEqual(
    upsertToastItem([item(1, "a"), item(2, "b")], item(3, "a")).map((entry) => entry.id),
    [2, 3],
  );
});

test("resolveToastStackClassName：四种位置都带 fixed 与层级", () => {
  for (const position of ["top-center", "top-right", "bottom-left", "bottom-center"] as const) {
    const className = resolveToastStackClassName(position);
    assert.match(className, /fixed/);
    assert.match(className, /z-\[9999\]/);
  }
  assert.match(resolveToastStackClassName("top-right"), /end-4/);
});

test("toast 队列：push/patch/remove 通知订阅者", () => {
  resetToasts();
  const seen: number[] = [];
  const unsubscribe = subscribeToasts((items) => seen.push(items.length));
  const id = pushToast("hello", { durationMs: 0 });
  assert.equal(getToastItems().length, 1);
  patchToast(id, { message: "updated" });
  assert.equal(getToastItems()[0]?.message, "updated");
  removeToast(id);
  assert.equal(getToastItems().length, 0);
  unsubscribe();
  assert.deepEqual(seen, [0, 1, 1, 0]);
});
