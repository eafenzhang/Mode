import assert from "node:assert/strict";
import test from "node:test";

import {
  clampIndex,
  filterSelectionItems,
  printableKey,
  selectionInputDisplayValue,
  visibleSelectionItemWindow,
} from "../src/app-selection-keyboard.js";

test("选择过滤：空过滤返回全部，非空按 keywords 大小写不敏感子串匹配", () => {
  const items = [{ keywords: ["alphabet"] }, { keywords: ["beta"] }, { keywords: ["ALPHA"] }];
  assert.deepEqual(filterSelectionItems({ items, filter: "", selectedIndex: 0 } as never), items);
  assert.deepEqual(filterSelectionItems({ items, filter: "al", selectedIndex: 0 } as never), [
    items[0],
    items[2],
  ]);
  assert.deepEqual(filterSelectionItems({ items, filter: "zz", selectedIndex: 0 } as never), []);
});

test("选择过滤：无 keywords 的条目在非空过滤下被排除，过滤词优先于输入值", () => {
  const items = [{ value: "alpha" }, { keywords: ["alpha"] }];
  assert.deepEqual(filterSelectionItems({ items, filter: "al", selectedIndex: 0 } as never), [
    items[1],
  ]);
  assert.deepEqual(
    filterSelectionItems({
      items,
      filter: "zz",
      selectedIndex: 0,
      input: { value: "al", cursor: 2 },
    } as never),
    [],
  );
});

test("下标夹取：负值取首项、超界取末项、空列表为 0", () => {
  assert.equal(clampIndex(-1, 3), 0);
  assert.equal(clampIndex(9, 3), 2);
  assert.equal(clampIndex(0, 0), 0);
});

test("可视窗口：从选中项向前滚动并返回窗内下标", () => {
  const items = [{ keywords: ["a"] }, { keywords: ["b"] }, { keywords: ["c"] }];
  assert.deepEqual(visibleSelectionItemWindow(items as never, 2, 2), {
    items: [items[1], items[2]],
    selectedIndex: 1,
    startIndex: 1,
  });
});

test("可打印键：普通字符与空格产出文本，功能键与修饰键不产出", () => {
  assert.equal(printableKey({ name: "a" } as never), "a");
  assert.equal(printableKey({ name: "space" } as never), " ");
  assert.equal(printableKey({ name: "return", sequence: "\r" } as never), undefined);
  assert.equal(printableKey({ name: "tab", sequence: "\t" } as never), undefined);
  assert.equal(printableKey({ name: "a", ctrl: true } as never), undefined);
});

test("输入展示：直接返回输入值", () => {
  assert.equal(selectionInputDisplayValue({ value: "hello", cursor: 2 } as never), "hello");
  assert.equal(selectionInputDisplayValue({ value: "" } as never), "");
});
