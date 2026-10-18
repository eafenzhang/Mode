/**
 * M4 adapter：Select 根状态、开启条件与选项选中语义。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../src/ui-foundation/adapters/select.js";

function renderSelect(props: { defaultOpen?: boolean; defaultValue?: string }): string {
  return renderToStaticMarkup(
    createElement(
      Select,
      props,
      createElement(
        SelectTrigger,
        { "aria-label": "source" },
        createElement(SelectValue, { placeholder: "Choose" }),
      ),
      createElement(
        SelectContent,
        null,
        createElement(SelectItem, { value: "a" }, "Alpha"),
        createElement(SelectItem, { value: "b" }, "Beta"),
      ),
    ),
  );
}

test("Select adapter：关闭时 listbox 隐藏且触发器未展开", () => {
  const html = renderSelect({});
  assert.match(html, /aria-expanded="false"/);
  assert.match(html, /hidden=""/);
  assert.match(html, /aria-selected="false"/);
  assert.match(html, /Choose/);
});

test("Select adapter：defaultOpen 展开并标记选中项", () => {
  const html = renderSelect({ defaultOpen: true, defaultValue: "b" });
  assert.match(html, /aria-expanded="true"/);
  assert.doesNotMatch(html, /hidden=""/);
  const selected = html.match(/<button[^>]*aria-selected="true"[^>]*>([\s\S]*?)<\/button>/);
  assert.ok(selected, "必须有一个选中项");
  assert.match(selected[1] ?? "", /Beta/);
});
