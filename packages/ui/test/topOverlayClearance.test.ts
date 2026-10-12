/** 顶部浮层左上入口的避让内边距（spec: lan-remote-desktop UI 规则·会话面「左侧栏收起时操作栏避让」）。
 * 运行：pnpm exec tsx --tsconfig packages/ui/tsconfig.json --test packages/ui/test/topOverlayClearance.test.ts
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { resolveTopOverlayClearancePaddingClass } from "../src/lib/topOverlayClearance.js";

test("左侧栏展开：浮层限制在侧栏宽度内，主区顶栏不避让", () => {
  assert.equal(resolveTopOverlayClearancePaddingClass({ isSidebarVisible: true }), null);
  assert.equal(
    resolveTopOverlayClearancePaddingClass({
      isSidebarVisible: true,
      isMacDesktop: true,
      hasUpdateReady: true,
    }),
    null,
  );
});

test("左侧栏收起（Windows/Linux/Web）：ps-38，有更新入口时 ps-44", () => {
  assert.equal(resolveTopOverlayClearancePaddingClass({ isSidebarVisible: false }), "ps-38");
  assert.equal(
    resolveTopOverlayClearancePaddingClass({ isSidebarVisible: false, hasUpdateReady: true }),
    "ps-44",
  );
});

test("左侧栏收起（macOS 非全屏）：额外让出红绿灯占位 ps-58 / ps-66", () => {
  assert.equal(
    resolveTopOverlayClearancePaddingClass({ isSidebarVisible: false, isMacDesktop: true }),
    "ps-58",
  );
  assert.equal(
    resolveTopOverlayClearancePaddingClass({
      isSidebarVisible: false,
      isMacDesktop: true,
      hasUpdateReady: true,
    }),
    "ps-66",
  );
});

test("左侧栏收起（macOS 全屏）：无红绿灯占位，ps-38 / ps-48", () => {
  assert.equal(
    resolveTopOverlayClearancePaddingClass({
      isSidebarVisible: false,
      isMacDesktop: true,
      isMacFullscreen: true,
    }),
    "ps-38",
  );
  assert.equal(
    resolveTopOverlayClearancePaddingClass({
      isSidebarVisible: false,
      isMacDesktop: true,
      isMacFullscreen: true,
      hasUpdateReady: true,
    }),
    "ps-48",
  );
});
