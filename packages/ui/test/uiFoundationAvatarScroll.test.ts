/**
 * M4 adapter map §1 缺口补漏：Avatar / ScrollArea。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import { findUiTypographyViolations } from "../src/lib/uiFoundation.js";
import {
  avatarBadgeClassName,
  avatarClassName,
  avatarFallbackClassName,
  avatarGroupClassName,
  avatarImageClassName,
} from "../src/ui-foundation/avatarStyles.js";
import {
  scrollAreaClassName,
  scrollAreaViewportClassName,
  scrollBarClassName,
} from "../src/ui-foundation/scrollStyles.js";

test("Avatar：尺寸档位、圆形、fallback 与图片裁切", () => {
  assert.match(avatarClassName("sm"), /size-6/);
  assert.match(avatarClassName(), /size-8/);
  assert.match(avatarClassName("lg"), /size-10/);
  assert.match(avatarClassName(), /rounded-full/);
  assert.match(avatarImageClassName(), /object-cover/);
  assert.match(avatarFallbackClassName(), /bg-surface/);
  assert.match(avatarGroupClassName(), /items-center/);
});

test("AvatarBadge：状态色与描边", () => {
  assert.match(avatarBadgeClassName("success"), /bg-success/);
  assert.match(avatarBadgeClassName("destructive"), /bg-destructive/);
  assert.match(avatarBadgeClassName(), /border-background/);
});

test("ScrollArea：容器裁剪、viewport 滚动、滚动条装饰", () => {
  assert.match(scrollAreaClassName(), /relative/);
  assert.match(scrollAreaClassName(), /overflow-hidden/);
  assert.match(scrollAreaViewportClassName(), /overflow-auto/);

  const vertical = scrollBarClassName("vertical");
  assert.match(vertical, /h-full/);
  assert.match(vertical, /w-2/);
  assert.match(vertical, /pointer-events-none/);
  const horizontal = scrollBarClassName("horizontal");
  assert.match(horizontal, /h-2/);
  assert.match(horizontal, /w-full/);
});

test("Avatar/ScrollArea 类名通过排版守卫", () => {
  for (const classes of [
    avatarClassName(),
    avatarImageClassName(),
    avatarFallbackClassName(),
    avatarGroupClassName(),
    avatarBadgeClassName(),
    scrollAreaClassName(),
    scrollAreaViewportClassName(),
    scrollBarClassName("vertical"),
    scrollBarClassName("horizontal"),
  ]) {
    assert.deepEqual(findUiTypographyViolations([classes]), []);
  }
});
