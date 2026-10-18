/**
 * M4 adapter：HoverCard 开关条件与浮层内容。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

import {
  HoverCard,
  HoverCardContent,
  HoverCardTrigger,
} from "../src/ui-foundation/adapters/hover-card.js";

function renderHoverCard(defaultOpen: boolean): string {
  return renderToStaticMarkup(
    createElement(
      HoverCard,
      { defaultOpen },
      createElement(
        HoverCardTrigger,
        { asChild: true },
        createElement("span", { className: "trigger" }, "Hover"),
      ),
      createElement(HoverCardContent, { align: "start" }, "card-body"),
    ),
  );
}

test("HoverCard adapter：默认关闭不渲染内容", () => {
  const html = renderHoverCard(false);
  assert.match(html, /aria-expanded="false"/);
  assert.doesNotMatch(html, /card-body/);
});

test("HoverCard adapter：打开时渲染可悬停浮层", () => {
  const html = renderHoverCard(true);
  assert.match(html, /data-slot="hover-card-content"/);
  assert.match(html, /card-body/);
  assert.match(html, /p-4/);
  assert.match(html, /aria-expanded="true"/);
});
