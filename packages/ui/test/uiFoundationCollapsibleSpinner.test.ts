/**
 * M4 P1.1：Collapsible / Spinner 契约（spec: docs/specs/m4-ui-foundation.md §2.7）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "../src/ui-foundation/Collapsible.js";
import { Spinner } from "../src/ui-foundation/Spinner.js";

function renderCollapsible(props: { defaultOpen?: boolean; forceMount?: boolean }): string {
  return renderToStaticMarkup(
    createElement(
      Collapsible,
      { defaultOpen: props.defaultOpen },
      createElement(
        CollapsibleTrigger,
        { asChild: true },
        createElement("span", { className: "trigger" }, "Toggle"),
      ),
      createElement(CollapsibleContent, { forceMount: props.forceMount }, "body"),
    ),
  );
}

test("Collapsible：关闭时内容不渲染，触发器 aria-expanded=false", () => {
  const html = renderCollapsible({});
  assert.match(html, /aria-expanded="false"/);
  assert.match(html, /data-state="closed"/);
  assert.doesNotMatch(html, />body</);
});

test("Collapsible：defaultOpen 渲染内容与 open 状态", () => {
  const html = renderCollapsible({ defaultOpen: true });
  assert.match(html, /aria-expanded="true"/);
  assert.match(html, /data-state="open"/);
  assert.match(html, />body</);
});

test("Collapsible：forceMount 关闭时保留 DOM 并标 closed", () => {
  const html = renderCollapsible({ forceMount: true });
  assert.match(html, />body</);
  assert.match(html, /data-slot="collapsible-content"/);
  assert.match(html, /data-state="closed"/);
});

test("Spinner：默认装饰态，animate-spin 且不入 Tab 顺序", () => {
  const html = renderToStaticMarkup(createElement(Spinner, { className: "size-4" }));
  assert.match(html, /animate-spin/);
  assert.match(html, /aria-hidden="true"/);
  assert.match(html, /focusable="false"/);
  assert.doesNotMatch(html, /role="status"/);
});

test("Spinner：提供 label 时成为 status", () => {
  const html = renderToStaticMarkup(createElement(Spinner, { label: "Loading" }));
  assert.match(html, /role="status"/);
  assert.match(html, /aria-label="Loading"/);
  assert.doesNotMatch(html, /aria-hidden/);
});
