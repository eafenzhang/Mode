/**
 * M4 adapter map §1 缺口补漏：RadioGroup / Progress。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import { findUiTypographyViolations } from "../src/lib/uiFoundation.js";
import {
  progressIndicatorClassName,
  progressPercent,
  progressTrackClassName,
  radioClassName,
  radioGroupClassName,
} from "../src/ui-foundation/formStyles.js";

test("RadioGroup：垂直排列；radio 圆形 + 焦点环", () => {
  assert.match(radioGroupClassName(), /flex-col/);
  assert.match(radioGroupClassName(), /gap-2/);
  const radio = radioClassName();
  assert.match(radio, /size-4/);
  assert.match(radio, /rounded-full/);
  assert.match(radio, /focus-visible:ring-2/);
});

test("Progress 百分比：clamp、非法值与除零保护", () => {
  assert.equal(progressPercent(0, 100), 0);
  assert.equal(progressPercent(50, 100), 50);
  assert.equal(progressPercent(100, 100), 100);
  assert.equal(progressPercent(150, 100), 100);
  assert.equal(progressPercent(-10, 100), 0);
  assert.equal(progressPercent(5, 0), 0);
  assert.equal(progressPercent(Number.NaN, 100), 0);
  assert.equal(progressPercent(1, 4), 25);
});

test("Progress 轨道与指示条", () => {
  assert.match(progressTrackClassName(), /h-1\.5/);
  assert.match(progressTrackClassName(), /rounded-full/);
  assert.match(progressTrackClassName(), /bg-surface/);
  assert.match(progressIndicatorClassName(), /bg-primary/);
  assert.match(progressIndicatorClassName(), /transition-all/);
});

test("Radio/Progress 类名通过排版守卫", () => {
  for (const classes of [
    radioGroupClassName(),
    radioClassName(),
    progressTrackClassName(),
    progressIndicatorClassName(),
  ]) {
    assert.deepEqual(findUiTypographyViolations([classes]), []);
  }
});
