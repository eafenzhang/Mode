import assert from "node:assert/strict";
import test from "node:test";

import { diffViewForWidth } from "../src/app-shiki-diff-view.js";

test("diff 视图：宽终端 121 列起分栏，120 列及以下统一视图", () => {
  assert.equal(diffViewForWidth(0), "unified");
  assert.equal(diffViewForWidth(80), "unified");
  assert.equal(diffViewForWidth(120), "unified");
  assert.equal(diffViewForWidth(121), "split");
  assert.equal(diffViewForWidth(200), "split");
});
