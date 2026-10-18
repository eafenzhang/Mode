/**
 * M4 P1.1 输入类控件类名契约（spec: docs/specs/m4-ui-foundation.md §2.2/§5）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import { findUiTypographyViolations } from "../src/lib/uiFoundation.js";
import {
  inputClassName,
  selectTriggerClassName,
  textareaClassName,
} from "../src/ui-foundation/inputStyles.js";

test("Input 使用输入语义 token 与 hover/focus 状态", () => {
  const classes = inputClassName();
  assert.match(classes, /bg-input/);
  assert.match(classes, /border-input-border/);
  assert.match(classes, /hover:border-input-border-hover/);
  assert.match(classes, /focus:border-input-border-focused/);
  assert.match(classes, /focus:bg-input-focused/);
  assert.match(classes, /placeholder:text-foreground-subtlest/);
});

test("Input 仅真实错误时用 destructive 边框，并随父容器降级圆角", () => {
  assert.doesNotMatch(inputClassName(), /border-destructive/);
  assert.match(inputClassName({ invalid: true }), /border-destructive/);
  assert.match(inputClassName({ parentRadius: "none" }), /rounded-lg/);
  assert.match(inputClassName({ parentRadius: "lg" }), /rounded-md/);
  assert.match(inputClassName({ parentRadius: "md" }), /rounded-sm/);
});

test("Textarea 不固定高度，保持输入语义", () => {
  const classes = textareaClassName();
  assert.match(classes, /min-h-16/);
  assert.doesNotMatch(classes, /\bh-8\b/);
  assert.match(classes, /focus:bg-input-focused/);
});

test("Select trigger 保持输入框外观并声明展开态", () => {
  const classes = selectTriggerClassName({ open: true });
  assert.match(classes, /inline-flex/);
  assert.match(classes, /justify-between/);
  assert.match(classes, /border-input-border-focused/);
  assert.match(classes, /cursor-pointer/);
});

test("输入类控件的类名通过排版守卫", () => {
  for (const classes of [inputClassName(), textareaClassName(), selectTriggerClassName()]) {
    assert.deepEqual(findUiTypographyViolations([classes]), []);
  }
});
