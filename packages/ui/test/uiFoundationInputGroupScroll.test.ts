/**
 * M4 P1.1：InputGroup / ScrollFadeViewport 契约
 * （spec: docs/specs/m4-ui-foundation.md §2.11–2.12）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
} from "../src/ui-foundation/adapters/input-group.js";
import { ScrollFadeViewport } from "../src/ui-foundation/adapters/scroll-fade-viewport.js";

test("InputGroup：容器承担边框/焦点态，组内 input 无独立边框", () => {
  const html = renderToStaticMarkup(
    createElement(
      InputGroup,
      null,
      createElement(
        InputGroupAddon,
        { align: "inline-start" },
        createElement(InputGroupButton, { size: "icon-xs", "aria-label": "minus" }, "-"),
      ),
      createElement(InputGroupInput, { "aria-label": "value", defaultValue: "3" }),
    ),
  );
  assert.match(html, /data-slot="input-group"/);
  assert.match(html, /focus-within:border-input-border-focused/);
  assert.match(html, /data-align="inline-start"/);
  assert.match(html, /data-slot="input-group-input"/);
  const input = html.match(/<input[^>]*data-slot="input-group-input"[^>]*>/)?.[0] ?? "";
  assert.doesNotMatch(input, /border-input-border/);
});

test("ScrollFadeViewport：初始无遮罩，className 与事件照常透传", () => {
  const html = renderToStaticMarkup(
    createElement(
      ScrollFadeViewport,
      { className: "h-full overflow-auto", "data-testid": "scroll" },
      createElement("p", null, "line"),
    ),
  );
  assert.match(html, /data-slot="scroll-fade-viewport"/);
  assert.match(html, /h-full overflow-auto/);
  assert.doesNotMatch(html, /mask-image/);
  assert.match(html, /line/);
});
