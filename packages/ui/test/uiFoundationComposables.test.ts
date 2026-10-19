/**
 * M4 adapter map §5 composable 补漏：Dialog / Menu / Select。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

import { findUiTypographyViolations } from "../src/lib/uiFoundation.js";
import { SelectTrigger } from "../src/ui-foundation/SelectTrigger.js";
import {
  selectContentClassName,
  selectGroupLabelClassName,
  selectItemClassName,
  selectValueClassName,
} from "../src/ui-foundation/selectStyles.js";
import {
  dialogDescriptionClassName,
  dialogFooterClassName,
  dialogHeaderClassName,
  dialogTitleClassName,
  menuGroupClassName,
  menuItemClassName,
} from "../src/ui-foundation/surfaceStyles.js";

test("Dialog 子件：header/title/description/footer", () => {
  assert.match(dialogHeaderClassName(), /flex-col/);
  assert.match(dialogHeaderClassName(), /gap-1\.5/);
  assert.match(dialogTitleClassName(), /text-ui-lg/);
  assert.match(dialogTitleClassName(), /font-medium/);
  assert.match(dialogDescriptionClassName(), /text-ui-sm/);
  assert.match(dialogDescriptionClassName(), /text-foreground-subtle/);
  assert.match(dialogFooterClassName(), /justify-end/);
});

test("MenuItem 给未显式指定尺寸的图标兜底 size-4", () => {
  const item = menuItemClassName();
  assert.ok(
    item.includes("[&_svg:not([class*='size-']):not([class*='w-']):not([class*='h-'])]:size-4"),
  );
});

test("MenuGroup：紧凑垂直排列", () => {
  const group = menuGroupClassName();
  assert.match(group, /flex-col/);
  assert.match(group, /gap-0\.5/);
});

test("Select 内容/选项/组/值：菜单 shell 语言与占位色", () => {
  const content = selectContentClassName();
  assert.match(content, /bg-menu/);
  assert.match(content, /rounded-lg/);
  assert.match(content, /max-h-60/);
  assert.match(content, /overflow-y-auto/);
  assert.match(selectItemClassName(), /aria-selected:font-medium/);
  assert.match(selectGroupLabelClassName(), /text-ui-xs/);
  assert.match(selectValueClassName({ placeholder: true }), /text-foreground-subtlest/);
  assert.match(selectValueClassName(), /truncate/);
  assert.match(selectValueClassName(), /text-foreground/);
});

test("SelectTrigger 的 className 可覆盖基础宽度/内边距", () => {
  const html = renderToStaticMarkup(
    createElement(SelectTrigger, { className: "w-auto px-1.5" }, "开启"),
  );
  assert.match(html, /w-auto/);
  assert.doesNotMatch(html, /(^|["\s])w-full([" \s])/);
  assert.match(html, /px-1\.5/);
});

test("SelectTrigger 内部图标与文字同一行，不被拆成两行", () => {
  const html = renderToStaticMarkup(
    createElement(SelectTrigger, null, createElement("svg", null), "开启"),
  );
  assert.match(html, /class="flex min-w-0 flex-1 items-center gap-2 overflow-hidden"/);
});

test("composable 类名通过排版守卫", () => {
  for (const classes of [
    dialogHeaderClassName(),
    dialogTitleClassName(),
    dialogDescriptionClassName(),
    dialogFooterClassName(),
    menuGroupClassName(),
    selectContentClassName(),
    selectItemClassName(),
    selectGroupLabelClassName(),
    selectValueClassName({ placeholder: true }),
  ]) {
    assert.deepEqual(findUiTypographyViolations([classes]), []);
  }
});
