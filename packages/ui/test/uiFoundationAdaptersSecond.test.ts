/**
 * M4 adapter 第二批：dropdown-menu / select / tooltip 子件导出。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import { findUiTypographyViolations } from "../src/lib/uiFoundation.js";
import {
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "../src/ui-foundation/adapters/index.js";
import { menuLabelClassName } from "../src/ui-foundation/surfaceStyles.js";

test("dropdown-menu adapter 子件齐备（旧导出名）", () => {
  for (const component of [
    DropdownMenuContent,
    DropdownMenuGroup,
    DropdownMenuItem,
    DropdownMenuCheckboxItem,
    DropdownMenuRadioItem,
    DropdownMenuSeparator,
  ]) {
    assert.ok(component, "组件必须存在");
  }
  assert.equal(typeof DropdownMenuLabel, "function");
});

test("select adapter 子件齐备", () => {
  for (const component of [SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue]) {
    assert.ok(component);
  }
  assert.equal(typeof SelectLabel, "function");
});

test("tooltip adapter 子件齐备", () => {
  assert.ok(TooltipContent);
  assert.equal(typeof TooltipProvider, "function");
  assert.equal(typeof TooltipTrigger, "function");
});

test("菜单 label 类名通过排版守卫", () => {
  assert.match(menuLabelClassName(), /text-ui-xs/);
  assert.deepEqual(findUiTypographyViolations([menuLabelClassName()]), []);
});

test("Avatar/ScrollArea adapter 导出存在", async () => {
  const adapters = await import("../src/ui-foundation/adapters/index.js");
  for (const name of [
    "Avatar",
    "AvatarImage",
    "AvatarFallback",
    "AvatarBadge",
    "AvatarGroup",
    "AvatarGroupCount",
    "ScrollArea",
    "ScrollBar",
  ] as const) {
    assert.ok(adapters[name], `${name} 必须导出`);
  }
});

test("剩余基础件 adapter 导出存在", async () => {
  const adapters = await import("../src/ui-foundation/adapters/index.js");
  for (const name of [
    "Alert",
    "AlertTitle",
    "AlertDescription",
    "AlertAction",
    "Textarea",
    "Checkbox",
    "Switch",
    "RadioGroup",
    "Radio",
    "Progress",
  ] as const) {
    assert.ok(adapters[name], `${name} 必须导出`);
  }
});
