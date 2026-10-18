/**
 * M4 P1.1 Card/Menu/Popover/Dialog 类名契约（spec: docs/specs/m4-ui-foundation.md §2.3/§2.4）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import { findUiTypographyViolations } from "../src/lib/uiFoundation.js";
import {
  cardClassName,
  dialogClassName,
  menuClassName,
  menuItemClassName,
  popoverClassName,
} from "../src/ui-foundation/surfaceStyles.js";

test("Card 使用 card/surface token，并按嵌套层级降半径", () => {
  const base = cardClassName();
  assert.match(base, /bg-card/);
  assert.match(base, /border-card-border/);
  assert.match(base, /rounded-xl/);
  assert.match(base, /p-4/);

  assert.match(cardClassName({ parentRadius: "xl" }), /rounded-lg/);
  assert.match(cardClassName({ parentRadius: "lg" }), /rounded-md/);
  assert.match(cardClassName({ parentRadius: "md" }), /rounded-sm/);
  assert.match(cardClassName({ lowEmphasis: true }), /bg-surface/);
  assert.match(cardClassName({ selected: true }), /bg-card-selected/);
  assert.doesNotMatch(cardClassName({ padded: false }), /\bp-4\b/);
});

test("Menu shell 与选项使用菜单语言", () => {
  const menu = menuClassName();
  assert.match(menu, /bg-menu/);
  assert.match(menu, /rounded-lg/);
  assert.match(menu, /shadow-md/);
  assert.match(menu, /p-1/);

  const item = menuItemClassName();
  assert.match(item, /rounded-md/);
  assert.match(item, /px-2/);
  assert.match(item, /text-ui-base/);
  assert.match(item, /hover:bg-menu-hover/);
  assert.match(menuItemClassName({ destructive: true }), /text-destructive/);
});

test("Popover 从 xl 起，Dialog shell 固定 2xl", () => {
  assert.match(popoverClassName(), /bg-popover/);
  assert.match(popoverClassName(), /rounded-xl/);
  assert.match(popoverClassName("lg"), /rounded-md/);
  assert.match(dialogClassName(), /rounded-2xl/);
  assert.match(dialogClassName(), /bg-popover/);
});

test("Card/Menu/Popover/Dialog 类名通过排版守卫", () => {
  for (const classes of [
    cardClassName(),
    menuClassName(),
    menuItemClassName(),
    popoverClassName(),
    dialogClassName(),
  ]) {
    assert.deepEqual(findUiTypographyViolations([classes]), []);
  }
});
