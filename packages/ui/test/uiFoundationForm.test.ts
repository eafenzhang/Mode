/**
 * M4 adapter map §1 缺口补漏：Alert / Checkbox / Switch。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import { findUiTypographyViolations } from "../src/lib/uiFoundation.js";
import {
  alertActionClassName,
  alertClassName,
  alertDescriptionClassName,
  alertTitleClassName,
} from "../src/ui-foundation/alertStyles.js";
import {
  checkboxClassName,
  checkboxRowClassName,
  switchThumbClassName,
  switchTrackClassName,
} from "../src/ui-foundation/formStyles.js";

test("Alert：语义 tone 与标题/说明/操作槽", () => {
  const neutral = alertClassName();
  assert.match(neutral, /bg-surface/);
  assert.match(neutral, /border-border/);
  assert.match(alertClassName({ tone: "destructive" }), /border-destructive/);
  assert.match(alertClassName({ tone: "destructive" }), /text-destructive/);
  assert.match(alertTitleClassName(), /text-ui-base/);
  assert.match(alertDescriptionClassName(), /text-ui-sm/);
  assert.match(alertActionClassName(), /ms-auto/);
});

test("Checkbox：尺寸/焦点环/禁用态", () => {
  const checkbox = checkboxClassName();
  assert.match(checkbox, /size-4/);
  assert.match(checkbox, /border-input-border/);
  assert.match(checkbox, /focus-visible:ring-2/);
  assert.match(checkbox, /disabled:opacity-50/);
  assert.match(checkboxRowClassName(), /items-center/);
});

test("Switch：checked 态换背景/位移，未选中为 input 背景", () => {
  assert.match(switchTrackClassName(true), /bg-primary/);
  assert.match(switchTrackClassName(false), /bg-input/);
  assert.match(switchThumbClassName(true), /translate-x-4/);
  assert.match(switchThumbClassName(false), /translate-x-0\.5/);
  assert.match(switchTrackClassName(true), /focus-visible:ring-2/);
});

test("Alert/Checkbox/Switch 类名通过排版守卫", () => {
  for (const classes of [
    alertClassName(),
    alertClassName({ tone: "warning" }),
    alertTitleClassName(),
    alertDescriptionClassName(),
    alertActionClassName(),
    checkboxClassName(),
    checkboxRowClassName(),
    switchTrackClassName(true),
    switchTrackClassName(false),
    switchThumbClassName(true),
    switchThumbClassName(false),
  ]) {
    assert.deepEqual(findUiTypographyViolations([classes]), []);
  }
});
