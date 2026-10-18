/**
 * M4 adapter：PptxSelectionActionBar 的定位与提交边界
 * （spec: docs/specs/m4-ui-foundation.md §2.19）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import {
  normalizePptxComment,
  resolvePptxActionBarPosition,
  shouldSubmitPptxComment,
} from "../src/ui-foundation/pptxSelectionState.js";

test("操作条水平居中并夹在视口内", () => {
  assert.deepEqual(resolvePptxActionBarPosition({ left: 100, top: 40, width: 200 }, 1000), {
    left: 200,
    top: 48,
  });
  assert.deepEqual(resolvePptxActionBarPosition({ left: -80, top: 0, width: 100 }, 1000), {
    left: 16,
    top: 8,
  });
});

test("评论提交：trim 后非空且未禁用", () => {
  assert.equal(normalizePptxComment("  hello  "), "hello");
  assert.equal(shouldSubmitPptxComment("   ", false), false);
  assert.equal(shouldSubmitPptxComment("hi", true), false);
  assert.equal(shouldSubmitPptxComment(" hi ", false), true);
});
