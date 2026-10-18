/**
 * M4 adapter：DropdownMenu 开启条件、条目语义与子菜单结构。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "../src/ui-foundation/adapters/dropdown-menu.js";

function renderMenu(props: { defaultOpen?: boolean; forceMount?: boolean }): string {
  return renderToStaticMarkup(
    createElement(
      DropdownMenu,
      { defaultOpen: props.defaultOpen },
      createElement(
        DropdownMenuTrigger,
        { asChild: true },
        createElement("button", { type: "button" }, "Open"),
      ),
      createElement(
        DropdownMenuContent,
        { forceMount: props.forceMount },
        createElement(DropdownMenuItem, { onSelect: () => undefined }, "Rename"),
        createElement(DropdownMenuItem, { variant: "destructive", inset: true }, "Delete"),
        createElement(DropdownMenuSeparator, null),
        createElement(
          DropdownMenuRadioGroup,
          { value: "b" },
          createElement(DropdownMenuRadioItem, { value: "a" }, "Alpha"),
          createElement(DropdownMenuRadioItem, { value: "b" }, "Beta"),
        ),
        createElement(DropdownMenuCheckboxItem, { checked: true }, "Pinned"),
        createElement(
          DropdownMenuSub,
          null,
          createElement(DropdownMenuSubTrigger, null, "More"),
          createElement(DropdownMenuSubContent, null, "Nested"),
        ),
      ),
    ),
  );
}

test("DropdownMenu adapter：默认关闭时不渲染内容", () => {
  const html = renderMenu({});
  assert.match(html, /aria-expanded="false"/);
  assert.doesNotMatch(html, /Rename/);
});

test("DropdownMenu adapter：defaultOpen 渲染条目、破坏性变体与选择态", () => {
  const html = renderMenu({ defaultOpen: true });
  assert.match(html, /aria-expanded="true"/);
  assert.match(html, /Rename/);
  assert.match(html, /data-variant="destructive"/);
  assert.match(html, /data-inset=""/);
  assert.match(html, /aria-checked="true"/);
  assert.match(html, /Pinned/);
  assert.match(html, /role="menuitemradio"/);
});

test("DropdownMenu adapter：forceMount 保留内容并标记 closed", () => {
  const html = renderMenu({ forceMount: true });
  assert.match(html, /Rename/);
  assert.match(html, /data-state="closed"/);
  assert.match(html, /hidden=""/);
});

test("DropdownMenu adapter：子菜单触发器与内容结构", () => {
  const html = renderMenu({ defaultOpen: true });
  assert.match(html, /aria-haspopup="menu"/);
  assert.match(html, /data-slot="dropdown-menu-sub-content"|role="menu"/);
});
