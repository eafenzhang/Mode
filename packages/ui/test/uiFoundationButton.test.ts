/**
 * M4 P1.1 Button 类名契约（spec: docs/specs/m4-ui-foundation.md §2.1/§5）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import { findUiTypographyViolations } from "../src/lib/uiFoundation.js";
import { buttonClassName } from "../src/ui-foundation/buttonStyles.js";

test("Button variant 使用语义 token", () => {
  assert.match(buttonClassName({ variant: "primary" }), /bg-primary/);
  assert.match(buttonClassName({ variant: "primary" }), /text-primary-foreground/);
  assert.match(buttonClassName({ variant: "outline" }), /border-border/);
  assert.match(buttonClassName({ variant: "ghost" }), /bg-transparent/);
  assert.match(buttonClassName({ variant: "destructive" }), /bg-destructive/);
  assert.match(buttonClassName({ variant: "link" }), /text-primary/);
});

test("Button size 使用 text-ui-* 与尺寸基线", () => {
  assert.match(buttonClassName({ size: "xs" }), /h-6/);
  assert.match(buttonClassName({ size: "xs" }), /text-ui-xs/);
  assert.match(buttonClassName({ size: "default" }), /h-8/);
  assert.match(buttonClassName({ size: "default" }), /text-ui-base/);
  assert.match(buttonClassName({ size: "icon" }), /size-8/);
  assert.match(buttonClassName({ size: "icon-lg" }), /size-9/);
});

test("Button 半径跟随最近父容器（控件表）", () => {
  assert.match(buttonClassName({ parentRadius: "none" }), /rounded-lg/);
  assert.match(buttonClassName({ parentRadius: "xl" }), /rounded-lg/);
  assert.match(buttonClassName({ parentRadius: "lg" }), /rounded-md/);
  assert.match(buttonClassName({ parentRadius: "md" }), /rounded-sm/);
  assert.match(buttonClassName({ parentRadius: "sm" }), /rounded-sm/);
});

test("Button 类名通过排版守卫（不出现内置字号/任意 px）", () => {
  const classes = buttonClassName({ variant: "primary", size: "default" });
  assert.deepEqual(findUiTypographyViolations([classes]), []);
});

test("Button 给未显式指定尺寸的 Lucide 图标兜底 size-4", () => {
  const classes = buttonClassName();
  assert.ok(
    classes.includes("[&_svg:not([class*='size-']):not([class*='w-']):not([class*='h-'])]:size-4"),
  );
  assert.match(classes, /\[&_svg\]:shrink-0/);
});

test("调用方 className 会覆盖基础尺寸，而不是被基础类压住", () => {
  const classes = buttonClassName({
    size: "default",
    className: "h-7 px-1.5 gap-1 w-auto",
  });
  assert.match(classes, /h-7/);
  assert.doesNotMatch(classes, /(^|\s)h-8(\s|$)/);
  assert.match(classes, /px-1\.5/);
  assert.doesNotMatch(classes, /(^|\s)px-3(\s|$)/);
  assert.match(classes, /gap-1/);
  assert.doesNotMatch(classes, /(^|\s)gap-2(\s|$)/);
  assert.match(classes, /w-auto/);
  assert.doesNotMatch(classes, /(^|\s)w-full(\s|$)/);
});
