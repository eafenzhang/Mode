/**
 * M4 P1.2 面板尺寸持久化契约（spec: docs/specs/m4-ui-foundation.md §3）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import {
  createMemoryPanelSizePersistence,
  resolveInitialPanelSize,
} from "../src/ui-foundation/panelSizePersistence.js";

const bounds = { min: 200, max: 480 };

test("初始尺寸：已保存值优先并 clamp；缺失用 fallback 并 clamp", () => {
  assert.equal(resolveInitialPanelSize(320, 100, bounds), 320);
  assert.equal(resolveInitialPanelSize(999, 100, bounds), 480);
  assert.equal(resolveInitialPanelSize(10, 100, bounds), 200);
  assert.equal(resolveInitialPanelSize(null, 300, bounds), 300);
  assert.equal(resolveInitialPanelSize(null, 999, bounds), 480);
});

test("内存持久化按 panel 读写，缺失返回 null", () => {
  const store = createMemoryPanelSizePersistence({ terminal: 240 });
  assert.equal(store.load("terminal"), 240);
  assert.equal(store.load("sidePane"), null);
  store.save("sidePane", 360);
  assert.equal(store.load("sidePane"), 360);
});
