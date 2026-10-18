/**
 * M4 adapter 第一批：旧命名 → ui-foundation 的映射契约。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import { findUiTypographyViolations } from "../src/lib/uiFoundation.js";
import {
  badgeVariants,
  buttonVariants,
  mapBadgeVariant,
  mapButtonSize,
  mapButtonVariant,
} from "../src/ui-foundation/adapters/index.js";
import { Card, CardTitle } from "../src/ui-foundation/adapters/card.js";
import { Separator } from "../src/ui-foundation/adapters/separator.js";

test("Button adapter：default 映射到 primary，其余 variant 同名", () => {
  assert.equal(mapButtonVariant("default"), "primary");
  assert.equal(mapButtonVariant("primary"), "primary");
  assert.equal(mapButtonVariant("destructive"), "destructive");
  assert.equal(mapButtonVariant(), "primary");
});

test("Button adapter：size 名称原样传递，class 输出走 foundation", () => {
  assert.equal(mapButtonSize("default"), "default");
  assert.equal(mapButtonSize("icon-lg"), "icon-lg");
  assert.match(buttonVariants(), /bg-primary/);
  assert.match(buttonVariants({ variant: "outline" }), /border-border/);
  assert.match(buttonVariants({ size: "xs" }), /h-6/);
});

test("Badge adapter：旧 variant 归一到 tone", () => {
  assert.equal(mapBadgeVariant("default"), "neutral");
  assert.equal(mapBadgeVariant("secondary"), "neutral");
  assert.equal(mapBadgeVariant("outline"), "neutral");
  assert.equal(mapBadgeVariant("destructive"), "destructive");
  assert.equal(mapBadgeVariant("success"), "success");
  assert.match(badgeVariants({ variant: "destructive" }), /text-destructive/);
  assert.match(badgeVariants({ variant: "outline", pill: true }), /rounded-full/);
});

test("Card/Separator 保留旧导出名；adapter class 通过排版守卫", () => {
  assert.equal(typeof Card, "object");
  assert.equal(typeof CardTitle, "object");
  assert.equal(typeof Separator, "object");
  assert.deepEqual(
    findUiTypographyViolations([buttonVariants(), badgeVariants()]),
    [],
  );
});
