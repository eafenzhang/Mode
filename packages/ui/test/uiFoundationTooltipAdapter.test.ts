/**
 * M4 adapter：Tooltip 显示条件与可访问性挂点（旧 Radix API 形状）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "../src/ui-foundation/adapters/tooltip.js";

function renderTooltip(props: { open?: boolean; defaultOpen?: boolean }): string {
  return renderToStaticMarkup(
    createElement(
      TooltipProvider,
      { delayDuration: 600 },
      createElement(
        Tooltip,
        props,
        createElement(
          TooltipTrigger,
          { asChild: true },
          createElement("span", { className: "trigger" }, "Hover me"),
        ),
        createElement(TooltipContent, { side: "right", align: "start" }, "tip-body"),
      ),
    ),
  );
}

test("Tooltip adapter：默认关闭时只渲染触发器", () => {
  const html = renderTooltip({});
  assert.match(html, /Hover me/);
  assert.doesNotMatch(html, /tip-body/);
  assert.match(html, /data-state="closed"/);
  assert.doesNotMatch(html, /aria-describedby/);
});

test("Tooltip adapter：打开时渲染内容并连上 aria-describedby", () => {
  const html = renderTooltip({ defaultOpen: true });
  assert.match(html, /role="tooltip"/);
  assert.match(html, /tip-body/);
  assert.match(html, /data-side="right"/);
  assert.match(html, /data-align="start"/);
  assert.match(html, /data-state="open"/);
  assert.match(html, /aria-describedby="[^"]+"/);
});

test("Tooltip adapter：受控 open=false 覆盖 defaultOpen", () => {
  const html = renderTooltip({ open: false, defaultOpen: true });
  assert.doesNotMatch(html, /tip-body/);
});
