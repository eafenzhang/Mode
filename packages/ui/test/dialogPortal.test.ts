import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

// Dialog portal 语义（docs/specs/m4-ui-foundation.md §2.4）：
// 遮罩/面板必须挂到 document.body，否则虚拟列表行的 transform 会把 fixed 遮罩
// 收编成行宽，侧栏/右侧面板不参与变暗。packages/ui 没有 DOM 环境，
// 这里用源码契约 + SSR 内联降级覆盖；真实 DOM 的 portal 目标由契约锁定。

const dialogSource = await readFile(
  new URL("../src/ui-foundation/adapters/dialog.tsx", import.meta.url),
  "utf8",
);
const { Dialog, DialogContent } = await import("../src/ui-foundation/adapters/dialog.js");

test("DialogContent 通过 DialogPortal 渲染，portal 目标是 document.body", () => {
  assert.ok(dialogSource.includes("<DialogPortal>"), "遮罩与面板必须经 DialogPortal 分发");
  assert.ok(
    dialogSource.includes("createPortal(children, document.body)"),
    "DialogPortal 必须 portal 到 document.body",
  );
  assert.ok(
    dialogSource.includes('typeof document === "undefined"'),
    "无 document 环境必须退回内联渲染",
  );
});

test("SSR 无 document 时内联降级，遮罩照常渲染", () => {
  const html = renderToStaticMarkup(
    createElement(
      Dialog,
      { open: true, onOpenChange: () => {} },
      createElement(DialogContent, null, createElement("p", null, "portal-body")),
    ),
  );
  assert.match(html, /data-slot="dialog-overlay"/);
  assert.match(html, /bg-black\/50/);
  assert.match(html, /portal-body/);
});

test("showOverlay=false 的用法保持无遮罩背景（TaskFindDialog / UpdateStatusWindowRoot）", () => {
  const html = renderToStaticMarkup(
    createElement(
      Dialog,
      { open: true, onOpenChange: () => {} },
      createElement(DialogContent, { showOverlay: false }, createElement("p", null, "no-overlay")),
    ),
  );
  assert.match(html, /data-slot="dialog-overlay"/);
  assert.doesNotMatch(html, /bg-black\/50/);
});
