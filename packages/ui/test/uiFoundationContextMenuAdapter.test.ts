/**
 * M4 adapter：ContextMenu 复用 DropdownMenu 条目与子菜单的状态契约。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
} from "../src/ui-foundation/adapters/context-menu.js";

function renderContextMenu(defaultOpen: boolean): string {
  return renderToStaticMarkup(
    createElement(
      ContextMenu,
      { defaultOpen },
      createElement(
        ContextMenuTrigger,
        { asChild: true },
        createElement("div", { className: "row" }, "Row"),
      ),
      createElement(
        ContextMenuContent,
        { className: "w-44" },
        createElement(ContextMenuItem, { onSelect: () => undefined }, "Open"),
        createElement(ContextMenuSeparator, null),
        createElement(
          ContextMenuSub,
          null,
          createElement(ContextMenuSubTrigger, null, "More"),
          createElement(ContextMenuSubContent, null, "Nested"),
        ),
      ),
    ),
  );
}

test("ContextMenu adapter：关闭时不渲染菜单，触发器带 menu 语义", () => {
  const html = renderContextMenu(false);
  assert.match(html, /aria-haspopup="menu"/);
  assert.doesNotMatch(html, />Open</);
});

test("ContextMenu adapter：打开时渲染条目、分隔线与子菜单", () => {
  const html = renderContextMenu(true);
  assert.match(html, /role="menu"/);
  assert.match(html, />Open</);
  assert.match(html, /role="separator"|my-1/);
  assert.match(html, />More</);
});
