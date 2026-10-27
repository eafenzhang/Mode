/**
 * 空态「创建工作区」入口与对话框（spec：docs/specs/workspace-creation.md）。
 *
 * 这里同时覆盖两层：
 * - 纯函数：名称校验与位置提示必须与 service 的 `~/ModeProject` 口径一致；
 * - 结构：入口在菜单固定组的第一项、选中标记在行尾，且创建能力由壳层注入。
 */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

import {
  ChatEmptyScratchWorkspaceDialog,
  getScratchWorkspaceLocationHint,
  getScratchWorkspaceNameErrorKind,
} from "../src/ChatEmptyScratchWorkspaceDialog.js";
import { ModeIntlProvider } from "../src/i18n/IntlProvider.js";
import enUS from "../src/i18n/locales/en-US.js";
import faIR from "../src/i18n/locales/fa.js";
import zhCN from "../src/i18n/locales/zh-CN.js";

const read = (relativePath: string): string =>
  readFileSync(new URL(relativePath, import.meta.url), "utf8");

test("工作区名称校验：空名称与路径分隔符分别报错", () => {
  assert.equal(getScratchWorkspaceNameErrorKind(""), "required");
  assert.equal(getScratchWorkspaceNameErrorKind("   "), "required");
  assert.equal(getScratchWorkspaceNameErrorKind("a/b"), "separator");
  assert.equal(getScratchWorkspaceNameErrorKind("a\\b"), "separator");
  assert.equal(getScratchWorkspaceNameErrorKind("demo"), null);
  assert.equal(getScratchWorkspaceNameErrorKind("  demo  "), null);
});

test("位置提示与 service 的 ~/ModeProject 口径一致", () => {
  assert.equal(getScratchWorkspaceLocationHint("demo"), "~/ModeProject/demo");
  assert.equal(getScratchWorkspaceLocationHint("  demo  "), "~/ModeProject/demo");
});

test("创建工作区入口在固定组第一项，打开文件夹保留在其后", () => {
  const source = read("../src/ChatEmptyState.tsx");
  const createIndex = source.indexOf('id: "chat.empty.createWorkspace.title"');
  const openIndex = source.indexOf('id: "workspace.openFolder"');

  assert.ok(createIndex >= 0, "空态菜单必须渲染「创建工作区」入口");
  assert.ok(openIndex > createIndex, "「创建工作区」必须在「打开文件夹」之前");
  assert.match(
    source,
    /data-testid=\{TID_COMPOSER_CREATE_WORKSPACE\}/,
    "入口必须有稳定的 testid，供 e2e 定位",
  );
});

test("工作区菜单的选中标记在行尾（原版格式）", () => {
  const source = read("../src/ChatEmptyState.tsx");
  assert.match(source, /indicatorPosition="end"/);
});

test("创建工作区入口只在壳层提供创建能力时渲染", () => {
  const source = read("../src/ChatEmptyState.tsx");
  assert.match(source, /\{onCreateWorkspace \? \(/, "缺少能力时不渲染入口");
  assert.match(source, /onCreate=\{onCreateWorkspace\}/, "对话框必须复用壳层注入的创建动作");

  const shell = read("../src/app-shell/WorkspaceShellLayout.tsx");
  assert.match(shell, /onCreateWorkspace=\{onCreateScratchWorkspace\}/);
});

test("对话框提交只委托 onCreate，不自行拼路径或写目录", () => {
  const source = read("../src/ChatEmptyScratchWorkspaceDialog.tsx");
  assert.match(source, /await onCreate\(trimmedName\)/);
  assert.match(source, /createdPath === null/, "当前模式不支持时必须走失败态");
  assert.match(source, /getScratchWorkspaceNameErrorKind\(name\)/);
});

test("对话框渲染名称、位置与操作按钮（zh-CN）", () => {
  const html = renderToStaticMarkup(
    createElement(
      ModeIntlProvider,
      { initialLocale: "zh-CN" },
      createElement(ChatEmptyScratchWorkspaceDialog, {
        open: true,
        onOpenChange: () => undefined,
        onCreate: async () => "/tmp/demo",
      }),
    ),
  );

  assert.match(html, /创建工作区/);
  assert.match(html, /工作区名称/);
  assert.match(html, /~\/ModeProject\//);
  assert.match(html, /取消/);
});

test("三套 locale 都有创建工作区文案", () => {
  const keys = [
    "chat.empty.createWorkspace.title",
    "chat.empty.createWorkspace.description",
    "chat.empty.createWorkspace.nameLabel",
    "chat.empty.createWorkspace.placeholder",
    "chat.empty.createWorkspace.locationLabel",
    "chat.empty.createWorkspace.locationHint",
    "chat.empty.createWorkspace.locationDescription",
    "chat.empty.createWorkspace.confirm",
  ] as const;

  for (const [localeName, messages] of [
    ["zh-CN", zhCN],
    ["en-US", enUS],
    ["fa-IR", faIR],
  ] as const) {
    for (const key of keys) {
      assert.equal(
        typeof messages[key],
        "string",
        `${localeName} 缺少 ${key}`,
      );
    }
  }
});
