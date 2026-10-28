import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { createElement, type ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";

import { TooltipProvider } from "../src/components/ui/tooltip.js";
import { ModeIntlProvider } from "../src/i18n/IntlProvider.js";
import { ConversationUserTextActions } from "../src/v4/ConversationUserTextActions.js";

const read = (relativePath: string): string =>
  readFileSync(new URL(relativePath, import.meta.url), "utf8");

function render(node: ReactNode): string {
  return renderToStaticMarkup(
    createElement(
      TooltipProvider,
      null,
      createElement(ModeIntlProvider, { initialLocale: "en-US" }, node),
    ),
  );
}

test("用户消息动作行常驻并显示发送时间", () => {
  const html = render(
    createElement(ConversationUserTextActions, {
      rowId: 7,
      text: "hello",
      createdAt: Date.now(),
      canEdit: true,
      onEdit: () => undefined,
    }),
  );
  assert.match(html, /v4-copy-7/);
  assert.match(html, /v4-edit-7/);
  assert.match(html, /\d{1,2}:\d{2}/);
  assert.doesNotMatch(html, /opacity-0/);
  assert.doesNotMatch(html, /group-hover\/user-row/);
});

test("用户消息没有编辑能力时不渲染编辑入口", () => {
  const html = render(
    createElement(ConversationUserTextActions, {
      rowId: 8,
      text: "hello",
      createdAt: Date.now(),
      canEdit: false,
    }),
  );
  assert.match(html, /v4-copy-8/);
  assert.doesNotMatch(html, /v4-edit-8/);
  assert.match(html, /\d{1,2}:\d{2}/);
});

test("助手动作行去掉 hover 门控并保留时间戳", () => {
  const rowView = read("../src/v4/ConversationRowView.tsx");
  assert.match(rowView, /formatMessageTimeLabel\(createdAt, locale, intl\)/);
  assert.doesNotMatch(rowView, /group-hover\/assistant-row/);
  assert.doesNotMatch(rowView, /opacity-0 transition-opacity group-hover\/user-row/);

  const turnGroup = read("../src/v4/ConversationTurnGroup.tsx");
  assert.doesNotMatch(turnGroup, /group-hover\/assistant-turn/);
});
