/**
 * M4 Dialog adapter：open/defaultOpen 组合的静态渲染契约。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "../src/ui-foundation/adapters/dialog.js";

function renderDialog(props: { open?: boolean; defaultOpen?: boolean }): string {
  return renderToStaticMarkup(
    createElement(
      Dialog,
      props,
      createElement(
        DialogContent,
        { labelledBy: "title" },
        createElement(DialogHeader, null, createElement(DialogTitle, null, "Title")),
      ),
    ),
  );
}

test("defaultOpen=true 渲染 dialog shell 与内容", () => {
  const html = renderDialog({ defaultOpen: true });
  assert.match(html, /role="dialog"/);
  assert.match(html, /aria-labelledby="title"/);
  assert.match(html, /Title/);
});

test("默认关闭时不渲染内容", () => {
  assert.equal(renderDialog({}), "");
});

test("受控 open=false 覆盖 defaultOpen", () => {
  assert.equal(renderDialog({ open: false, defaultOpen: true }), "");
});

test("受控 open=true 渲染内容", () => {
  assert.match(renderDialog({ open: true }), /Title/);
});
