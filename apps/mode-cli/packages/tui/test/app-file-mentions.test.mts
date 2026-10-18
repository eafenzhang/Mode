import assert from "node:assert/strict";
import test from "node:test";

import { visibleFileMentionWindow } from "../src/app-file-mentions.js";

const items = ["a", "b", "c", "d"];

test("提及窗口：空列表或非正可视数返回空窗口", () => {
  assert.deepEqual(visibleFileMentionWindow([], 0, 8), {
    items: [],
    selectedIndex: 0,
    startIndex: 0,
  });
  assert.deepEqual(visibleFileMentionWindow(items as never, 0, 0), {
    items: [],
    selectedIndex: 0,
    startIndex: 0,
  });
});

test("提及窗口：未超出可视数时从头展示", () => {
  assert.deepEqual(visibleFileMentionWindow(["a", "b"] as never, 1, 8), {
    items: ["a", "b"],
    selectedIndex: 1,
    startIndex: 0,
  });
});

test("提及窗口：选中项越界时夹取到末项并滚动窗口", () => {
  assert.deepEqual(visibleFileMentionWindow(items as never, 99, 2), {
    items: ["c", "d"],
    selectedIndex: 1,
    startIndex: 2,
  });
});

test("提及窗口：负下标夹取到首项", () => {
  assert.deepEqual(visibleFileMentionWindow(items as never, -5, 2), {
    items: ["a", "b"],
    selectedIndex: 0,
    startIndex: 0,
  });
});

test("提及窗口：可视数为 1 时只保留选中项", () => {
  assert.deepEqual(visibleFileMentionWindow(["a", "b", "c"] as never, 1, 1), {
    items: ["b"],
    selectedIndex: 0,
    startIndex: 1,
  });
});
