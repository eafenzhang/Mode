import assert from "node:assert/strict";
import test from "node:test";

import { createSelectionState } from "../src/app-selection-state.js";

test("选择状态：缺省运行时返回 undefined", () => {
  assert.equal(createSelectionState(undefined as never), undefined);
});

test("选择状态：默认过滤为空、选中首项", () => {
  assert.deepEqual(createSelectionState({ items: ["a", "b"] }), {
    items: ["a", "b"],
    filter: "",
    selectedIndex: 0,
  });
});

test("选择状态：选中下标夹在有效范围内", () => {
  assert.equal(createSelectionState({ items: ["a", "b", "c"], selectedIndex: 9 }).selectedIndex, 2);
  assert.equal(createSelectionState({ items: ["a", "b"], selectedIndex: -3 }).selectedIndex, 0);
  assert.equal(createSelectionState({ items: [] }).selectedIndex, 0);
});

test("选择状态：保留 cursor/query 等输入字段，filter 由状态自管", () => {
  const result = createSelectionState({ items: ["a"], cursor: 5, query: "qq", filter: "x" });
  assert.equal(result.cursor, 5);
  assert.equal(result.query, "qq");
  assert.equal(result.filter, "");
});
