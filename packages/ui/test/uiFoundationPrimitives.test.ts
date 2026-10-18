/**
 * M4 adapter map §5 基础件补漏：Card 子件 / Badge / Separator。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import { findUiTypographyViolations } from "../src/lib/uiFoundation.js";
import { badgeClassName } from "../src/ui-foundation/badgeStyles.js";
import { separatorClassName } from "../src/ui-foundation/separatorStyles.js";
import {
  cardActionClassName,
  cardContentClassName,
  cardDescriptionClassName,
  cardFooterClassName,
  cardHeaderClassName,
  cardTitleClassName,
} from "../src/ui-foundation/surfaceStyles.js";

test("Card 子件：header 两端、title base、description sm、footer/action", () => {
  assert.match(cardHeaderClassName(), /justify-between/);
  assert.match(cardTitleClassName(), /text-ui-base/);
  assert.match(cardTitleClassName(), /font-medium/);
  assert.match(cardDescriptionClassName(), /text-ui-sm/);
  assert.match(cardDescriptionClassName(), /text-foreground-subtle/);
  assert.match(cardContentClassName(), /min-w-0/);
  assert.match(cardFooterClassName(), /items-center/);
  assert.match(cardActionClassName(), /shrink-0/);
});

test("Badge：默认 neutral，语义色按 tone，pill 才用 rounded-full", () => {
  const neutral = badgeClassName();
  assert.match(neutral, /rounded-md/);
  assert.match(neutral, /text-foreground-subtle/);
  assert.match(neutral, /text-ui-xs/);
  assert.doesNotMatch(neutral, /rounded-full/);

  assert.match(badgeClassName({ tone: "success" }), /text-success/);
  assert.match(badgeClassName({ tone: "warning" }), /text-warning/);
  assert.match(badgeClassName({ tone: "destructive" }), /text-destructive/);
  assert.match(badgeClassName({ pill: true }), /rounded-full/);
});

test("Separator：横/纵尺寸与背景", () => {
  const horizontal = separatorClassName("horizontal");
  assert.match(horizontal, /h-px/);
  assert.match(horizontal, /w-full/);
  const vertical = separatorClassName("vertical");
  assert.match(vertical, /w-px/);
  assert.match(vertical, /self-stretch/);
  assert.match(horizontal, /bg-border/);
});

test("补漏件类名通过排版守卫", () => {
  for (const classes of [
    cardHeaderClassName(),
    cardTitleClassName(),
    cardDescriptionClassName(),
    cardContentClassName(),
    cardFooterClassName(),
    cardActionClassName(),
    badgeClassName(),
    badgeClassName({ tone: "success", pill: true }),
    separatorClassName("horizontal"),
    separatorClassName("vertical"),
  ]) {
    assert.deepEqual(findUiTypographyViolations([classes]), []);
  }
});
