/**
 * M4 P1.1：Resizable 面板/分隔条结构契约
 * （spec: docs/specs/m4-ui-foundation.md §2.14）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup,
} from "../src/ui-foundation/adapters/resizable.js";

function renderGroup(layoutId?: string): string {
  return renderToStaticMarkup(
    createElement(
      ResizablePanelGroup,
      layoutId ? { layoutId, panelIds: ["left", "right"], orientation: "vertical" } : {},
      createElement(
        ResizablePanel,
        { id: "left", minSize: "30%" },
        createElement("span", null, "L"),
      ),
      createElement(ResizableHandle, { className: "h-1 w-full" }),
      createElement(ResizablePanel, { id: "right" }, createElement("span", null, "R")),
    ),
  );
}

test("Resizable：面板 id、分隔条挂点与调用方类名", () => {
  const html = renderGroup();
  assert.match(html, /id="left"/);
  assert.match(html, /id="right"/);
  assert.match(html, /L/);
  assert.match(html, /R/);
  assert.match(html, /data-slot="resizable-handle"/);
  assert.match(html, /h-1 w-full/);
});

test("Resizable：layoutId 走持久化分支仍能渲染", () => {
  const html = renderGroup("workspace-body-layout");
  assert.match(html, /id="left"/);
  assert.match(html, /data-slot="resizable-handle"/);
});
