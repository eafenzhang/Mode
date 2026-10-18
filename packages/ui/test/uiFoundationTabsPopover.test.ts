/**
 * M4 adapter：Tabs / Popover 旧 API 契约（defaultValue、asChild、forceMount、定位）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

import { Popover, PopoverAnchor, PopoverContent } from "../src/ui-foundation/adapters/popover.js";
import { computePopoverPoint } from "../src/ui-foundation/adapters/popoverPosition.js";
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
  tabsListVariants,
} from "../src/ui-foundation/adapters/tabs.js";

function renderTabs(forceMount: boolean): string {
  return renderToStaticMarkup(
    createElement(
      Tabs,
      { defaultValue: "plugins" },
      createElement(
        TabsList,
        null,
        createElement(TabsTrigger, { value: "plugins" }, "Plugins"),
        createElement(TabsTrigger, { value: "mcps" }, "MCPs"),
      ),
      createElement(TabsContent, { value: "plugins" }, "plugins-body"),
      createElement(TabsContent, { value: "mcps", forceMount }, "mcps-body"),
    ),
  );
}

test("Tabs adapter：defaultValue 选中首项，非激活内容默认不渲染", () => {
  const html = renderTabs(false);
  assert.match(html, /role="tablist"/);
  assert.match(html, /aria-selected="true"/);
  assert.match(html, /aria-selected="false"/);
  assert.match(html, /data-state="active"/);
  assert.match(html, /plugins-body/);
  assert.doesNotMatch(html, /mcps-body/);
});

test("Tabs adapter：forceMount 保留非激活内容并标记 inactive", () => {
  const html = renderTabs(true);
  assert.match(html, /mcps-body/);
  assert.match(html, /data-state="inactive"/);
});

test("Tabs adapter：asChild 把 tab 语义合并到子元素上", () => {
  const html = renderToStaticMarkup(
    createElement(
      Tabs,
      { defaultValue: "one" },
      createElement(
        TabsList,
        { variant: "line" },
        createElement(
          TabsTrigger,
          { value: "one", asChild: true },
          createElement("a", { href: "#one", className: "legacy" }, "One"),
        ),
      ),
    ),
  );
  assert.match(html, /<a[^>]*role="tab"/);
  assert.match(html, /aria-selected="true"/);
  assert.match(html, /class="legacy"/);
  assert.match(html, /gap-1 bg-transparent/);
});

test("Tabs adapter：tabsListVariants 支持 line 变体", () => {
  assert.match(tabsListVariants("line"), /gap-1 bg-transparent/);
  assert.doesNotMatch(tabsListVariants("default"), /bg-transparent/);
});

function renderPopover(open: boolean, forceMount = false): string {
  return renderToStaticMarkup(
    createElement(
      Popover,
      { open },
      createElement(PopoverAnchor, { virtualRef: { current: null } }),
      createElement(
        PopoverContent,
        { align: "start", side: "bottom", sideOffset: 6, forceMount },
        "popover-body",
      ),
    ),
  );
}

test("Popover adapter：关闭时不渲染内容，forceMount 时渲染 closed 状态", () => {
  assert.equal(renderPopover(false), "");
  const html = renderPopover(false, true);
  assert.match(html, /popover-body/);
  assert.match(html, /data-state="closed"/);
});

test("Popover adapter：打开时输出 open 状态与 Radix CSS 变量", () => {
  const html = renderPopover(true);
  assert.match(html, /popover-body/);
  assert.match(html, /data-state="open"/);
  assert.match(html, /--radix-popover-trigger-width/);
  assert.match(html, /--radix-popover-content-available-height/);
});

test("computePopoverPoint：四边对齐与视口收敛", () => {
  const anchor = {
    top: 100,
    left: 200,
    right: 260,
    bottom: 130,
    width: 60,
    height: 30,
    x: 200,
    y: 100,
    toJSON: () => ({}),
  } as DOMRect;
  const bottomStart = computePopoverPoint({
    anchor,
    content: { width: 120, height: 80 },
    align: "start",
    side: "bottom",
    sideOffset: 6,
    alignOffset: 0,
    avoidCollisions: true,
    collisionPadding: 8,
    viewport: { width: 1000, height: 800 },
  });
  assert.equal(bottomStart.top, 136);
  assert.equal(bottomStart.left, 200);
  assert.equal(bottomStart.triggerWidth, 60);

  const topEnd = computePopoverPoint({
    anchor,
    content: { width: 120, height: 80 },
    align: "end",
    side: "top",
    sideOffset: 4,
    alignOffset: 0,
    avoidCollisions: true,
    collisionPadding: 8,
    viewport: { width: 1000, height: 800 },
  });
  assert.equal(topEnd.top, 16);
  assert.equal(topEnd.left, 140);

  const clamped = computePopoverPoint({
    anchor: { ...anchor, left: 2, right: 62 } as DOMRect,
    content: { width: 120, height: 80 },
    align: "start",
    side: "bottom",
    sideOffset: 0,
    alignOffset: 0,
    avoidCollisions: true,
    collisionPadding: 8,
    viewport: { width: 1000, height: 800 },
  });
  assert.equal(clamped.left, 8);
});
