/**
 * M4 P1.1：Chart 容器/配置变量与 tooltip 展示契约
 * （spec: docs/specs/m4-ui-foundation.md §2.13）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

import {
  ChartContainer,
  ChartTooltipContent,
  type ChartConfig,
} from "../src/ui-foundation/adapters/chart.js";

const CONFIG: ChartConfig = {
  tokens: { label: "Tokens", color: "var(--color-usage-chart-1)" },
};

test("ChartContainer：输出 data-chart 与 --color-<key> 变量", () => {
  const html = renderToStaticMarkup(
    createElement(
      ChartContainer,
      { config: CONFIG, id: "usage", responsive: false },
      createElement("div", { "data-chart-child": "true" }),
    ),
  );
  assert.match(html, /data-chart="chart-usage"/);
  assert.match(html, /--color-tokens: var\(--color-usage-chart-1\)/);
  assert.match(html, /data-chart-child="true"/);
});

test("ChartTooltipContent：active=false 或空 payload 不渲染", () => {
  const html = renderToStaticMarkup(
    createElement(
      ChartContainer,
      { config: CONFIG, responsive: false },
      createElement(ChartTooltipContent, { active: false, payload: [] }),
    ),
  );
  assert.doesNotMatch(html, /data-slot="chart-tooltip"/);
});

test("ChartTooltipContent：按 config 取 label 并应用 formatter", () => {
  const html = renderToStaticMarkup(
    createElement(
      ChartContainer,
      { config: CONFIG, responsive: false },
      createElement(ChartTooltipContent, {
        active: true,
        label: "2026-10-19",
        payload: [{ dataKey: "tokens", value: 42, color: "#123456" }],
        formatter: (value) => `value:${String(value)}`,
      }),
    ),
  );
  assert.match(html, /data-slot="chart-tooltip"/);
  assert.match(html, /2026-10-19/);
  assert.match(html, /Tokens/);
  assert.match(html, /value:42/);
});
