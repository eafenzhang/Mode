import assert from "node:assert/strict";
import test from "node:test";

import { removeQueuedInputs, upsertQueuedInput } from "../src/app-queued-inputs.js";

const QUEUE = [
  { id: "a", text: "one" },
  { id: "b", text: "two" },
];

test("排队输入：新 id 追加到末尾，原顺序不变", () => {
  const next = upsertQueuedInput(QUEUE, { id: "c", text: "three" });
  assert.notEqual(next, QUEUE);
  assert.deepEqual(next, [
    { id: "a", text: "one" },
    { id: "b", text: "two" },
    { id: "c", text: "three" },
  ]);
});

test("排队输入：同 id 原位更新；内容未变时返回原数组", () => {
  const updated = upsertQueuedInput(QUEUE, { id: "a", text: "new" });
  assert.deepEqual(updated, [
    { id: "a", text: "new" },
    { id: "b", text: "two" },
  ]);
  assert.equal(upsertQueuedInput(QUEUE, { id: "a", text: "one" }), QUEUE);
});

test("排队输入：preserveExistingText 保留非空旧文本，旧文本为空时采用新值", () => {
  assert.equal(
    upsertQueuedInput(QUEUE, { id: "a", text: "new" }, { preserveExistingText: true }),
    QUEUE,
  );
  assert.deepEqual(
    upsertQueuedInput(
      [{ id: "a", text: "" }],
      { id: "a", text: "new" },
      { preserveExistingText: true },
    ),
    [{ id: "a", text: "new" }],
  );
});

test("排队输入：按 id 批量移除；空 id 列表返回原数组", () => {
  assert.deepEqual(removeQueuedInputs(QUEUE, ["a"]), [{ id: "b", text: "two" }]);
  assert.equal(removeQueuedInputs(QUEUE, []), QUEUE);
  const untouched = removeQueuedInputs(QUEUE, ["missing"]);
  assert.notEqual(untouched, QUEUE);
  assert.deepEqual(untouched, QUEUE);
});
