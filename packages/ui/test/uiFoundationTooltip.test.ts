/**
 * M4 adapter map §5：Tooltip 延迟契约。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import {
  DEFAULT_TOOLTIP_DELAY_MS,
  MAX_TOOLTIP_DELAY_MS,
  resolveTooltipDelay,
} from "../src/ui-foundation/tooltipContext.js";

test("Tooltip 延迟：默认 500ms，非法值回默认，范围 0–2000 并取整", () => {
  assert.equal(resolveTooltipDelay(), DEFAULT_TOOLTIP_DELAY_MS);
  assert.equal(resolveTooltipDelay(Number.NaN), DEFAULT_TOOLTIP_DELAY_MS);
  assert.equal(resolveTooltipDelay(0), 0);
  assert.equal(resolveTooltipDelay(-10), 0);
  assert.equal(resolveTooltipDelay(9999), MAX_TOOLTIP_DELAY_MS);
  assert.equal(resolveTooltipDelay(320.6), 321);
});
