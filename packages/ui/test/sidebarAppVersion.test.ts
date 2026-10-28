import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

import { MODE_VERSION } from "@mode/shared";

import { SidebarAppVersion } from "../src/SidebarAppVersion.js";

test("侧栏版本号来自构建常量且始终渲染", () => {
  const html = renderToStaticMarkup(createElement(SidebarAppVersion));
  assert.match(html, /data-testid="sidebar-app-version"/);
  assert.ok(html.includes(`v${MODE_VERSION}`), `缺少版本号 v${MODE_VERSION}：${html}`);
  assert.ok(html.includes(`title="v${MODE_VERSION}"`));
});
