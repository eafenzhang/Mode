/**
 * M4 P1.1：FlipMetricValue 渲染契约（spec: docs/specs/m4-ui-foundation.md §2.17）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

import { FlipMetricValue } from "../src/ui-foundation/adapters/flip-metric-value.js";

test("FlipMetricValue：aria-label 提供完整值，逐字符渲染", () => {
  const html = renderToStaticMarkup(
    createElement(FlipMetricValue, { value: "12.3k", className: "text-ui-base" }),
  );
  assert.match(html, /aria-label="12\.3k"/);
  assert.match(html, /tabular-nums/);
  assert.match(html, /text-ui-base/);
  for (const char of ["1", "2", ".", "3", "k"]) {
    assert.ok(html.includes(`>${char}<`), `缺少字符 ${char}`);
  }
});

test("FlipMetricValue：空格用不换行空格占位，animateInitial 打标", () => {
  const html = renderToStaticMarkup(
    createElement(FlipMetricValue, { value: "1 2", animateInitial: true }),
  );
  assert.match(html, /\u00a0/);
  assert.match(html, /data-animate-initial="true"/);
});
