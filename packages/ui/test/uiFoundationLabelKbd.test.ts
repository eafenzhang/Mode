/**
 * M4 P1.1：Label / Kbd 契约（spec: docs/specs/m4-ui-foundation.md §2.9）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

import { Kbd, KbdGroup } from "../src/ui-foundation/adapters/kbd.js";
import { Label } from "../src/ui-foundation/adapters/label.js";
import { findUiTypographyViolations } from "../src/lib/uiFoundation.js";

test("Label：语义 label 与排版 token，调用方 className 追加", () => {
  const html = renderToStaticMarkup(
    createElement(Label, { htmlFor: "name", className: "text-foreground-subtle" }, "Name"),
  );
  assert.match(html, /<label/);
  assert.match(html, /for="name"/);
  assert.match(html, /text-ui-sm/);
  assert.match(html, /text-foreground-subtle/);
});

test("Kbd / KbdGroup：组合键结构与 token", () => {
  const html = renderToStaticMarkup(
    createElement(KbdGroup, null, createElement(Kbd, null, "Ctrl"), createElement(Kbd, null, "K")),
  );
  assert.match(html, /<kbd/);
  assert.match(html, /text-ui-xs/);
  assert.match(html, /inline-flex items-center gap-1/);
  assert.deepEqual(
    findUiTypographyViolations([html.includes("text-ui-sm") ? "text-ui-sm" : "", "text-ui-xs"]),
    [],
  );
});
