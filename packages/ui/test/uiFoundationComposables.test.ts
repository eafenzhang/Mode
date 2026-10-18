/**
 * M4 adapter map §5 composable 补漏：Dialog / Menu / Select。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import { findUiTypographyViolations } from "../src/lib/uiFoundation.js";
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
