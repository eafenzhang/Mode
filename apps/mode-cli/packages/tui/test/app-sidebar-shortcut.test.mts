import assert from "node:assert/strict";
import test from "node:test";

import { createSidebarShortcutState, resolveSidebarShortcut } from "../src/app-sidebar-shortcut.js";

test("侧栏 leader：Ctrl+X 后 B/M/A 在窗口内触发对应动作并解除 leader", () => {
  const state = createSidebarShortcutState();
  assert.equal(resolveSidebarShortcut(state, { name: "x", ctrl: true }, 1000, 1000), "arm");
  assert.equal(resolveSidebarShortcut(state, { name: "b" }, 1100, 1000), "toggle");
  assert.equal(state.leaderArmedUntilMs, undefined);

  assert.equal(resolveSidebarShortcut(state, { name: "x", ctrl: true }, 2000, 1000), "arm");
  assert.equal(resolveSidebarShortcut(state, { name: "m" }, 2100, 1000), "toggle-files");

  assert.equal(resolveSidebarShortcut(state, { name: "x", ctrl: true }, 3000, 1000), "arm");
  assert.equal(resolveSidebarShortcut(state, { name: "a" }, 3100, 1000), "toggle-apis");
});

test("侧栏 leader：超时或其它键回退 pass 并清空 leader", () => {
  const state = createSidebarShortcutState();
  assert.equal(resolveSidebarShortcut(state, { name: "x", ctrl: true }, 1000, 1000), "arm");
  assert.equal(resolveSidebarShortcut(state, { name: "b" }, 2001, 1000), "pass");
  assert.equal(state.leaderArmedUntilMs, undefined);

  assert.equal(resolveSidebarShortcut(state, { name: "x", ctrl: true }, 3000, 1000), "arm");
  assert.equal(resolveSidebarShortcut(state, { name: "z" }, 3100, 1000), "pass");
  assert.equal(state.leaderArmedUntilMs, undefined);
});

test("侧栏 leader：带修饰键的 follow-up 不算切换键", () => {
  const state = createSidebarShortcutState();
  assert.equal(resolveSidebarShortcut(state, { name: "x", ctrl: true }, 1000, 1000), "arm");
  assert.equal(resolveSidebarShortcut(state, { name: "b", ctrl: true }, 1100, 1000), "pass");
});
