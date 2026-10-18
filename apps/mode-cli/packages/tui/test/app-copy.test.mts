import assert from "node:assert/strict";
import test from "node:test";

import { hasCopyableSelectionText, selectionCopyStatus } from "../src/app-copy.js";

const copy = {
  copied: "copied!",
  unavailable: "n/a",
  failed: "failed!",
  empty: "nothing",
} as never;

test("可复制选区：仅非空非空白字符串", () => {
  assert.equal(hasCopyableSelectionText(" x "), true);
  assert.equal(hasCopyableSelectionText("   "), false);
  assert.equal(hasCopyableSelectionText(null), false);
  assert.equal(hasCopyableSelectionText(undefined), false);
});

test("复制状态：copied/unavailable/failed 映射文案，failed 带详情，empty 不产出状态", () => {
  assert.deepEqual(selectionCopyStatus({ kind: "copied", characterCount: 2 }, copy), {
    details: [],
    status: "copied!",
  });
  assert.deepEqual(selectionCopyStatus({ kind: "unavailable" }, copy), {
    details: [],
    status: "n/a",
  });
  assert.deepEqual(selectionCopyStatus({ kind: "failed", message: "boom" }, copy), {
    details: ["boom"],
    status: "failed!",
  });
  assert.deepEqual(selectionCopyStatus({ kind: "empty" }, copy), {});
});
