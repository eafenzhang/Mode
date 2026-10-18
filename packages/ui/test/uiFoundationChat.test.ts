/**
 * M4 P1.3 会话核心类名契约（plan: docs/plans/2026-10-18-m4-ui-replacement.md P1.3）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import { findUiTypographyViolations } from "../src/lib/uiFoundation.js";
import {
  chatInputShellClassName,
  emptyStateClassName,
  messageBubbleClassName,
  messageListClassName,
  messageRowClassName,
  streamingIndicatorClassName,
} from "../src/ui-foundation/chatStyles.js";

test("消息列表：可滚动、16px 间距、正文容器", () => {
  const list = messageListClassName();
  assert.match(list, /overflow-y-auto/);
  assert.match(list, /gap-4/);
  assert.match(list, /min-h-0/);
  assert.match(list, /flex-1/);
});

test("角色对齐与气泡：用户右对齐卡片，助手无气泡底，工具用 surface+mono", () => {
  assert.match(messageRowClassName("user"), /justify-end/);
  assert.match(messageRowClassName("assistant"), /justify-start/);
  assert.match(messageRowClassName("tool"), /justify-start/);

  const user = messageBubbleClassName("user");
  assert.match(user, /bg-card/);
  assert.match(user, /rounded-xl/);
  assert.match(user, /text-ui-base/);

  const assistant = messageBubbleClassName("assistant");
  assert.doesNotMatch(assistant, /bg-card/);
  assert.match(assistant, /text-ui-base/);

  const tool = messageBubbleClassName("tool");
  assert.match(tool, /bg-surface/);
  assert.match(tool, /font-mono/);
});

test("空状态与流式指示使用文本层级：标题 base、说明 sm、指示 xs", () => {
  assert.match(emptyStateClassName(), /items-center/);
  assert.match(emptyStateClassName(), /justify-center/);
  assert.match(streamingIndicatorClassName(), /text-ui-xs/);
  assert.match(streamingIndicatorClassName(), /text-foreground-subtlest/);
});

test("主聊天输入 shell 使用批准的 rounded-2xl 例外", () => {
  const shell = chatInputShellClassName();
  assert.match(shell, /rounded-2xl/);
  assert.match(shell, /bg-input/);
  assert.match(shell, /focus-within:bg-input-focused/);
});

test("会话核心类名通过排版守卫", () => {
  for (const classes of [
    messageListClassName(),
    messageRowClassName("user"),
    messageBubbleClassName("user"),
    messageBubbleClassName("assistant"),
    messageBubbleClassName("system"),
    messageBubbleClassName("tool"),
    emptyStateClassName(),
    streamingIndicatorClassName(),
    chatInputShellClassName(),
  ]) {
    assert.deepEqual(findUiTypographyViolations([classes]), []);
  }
});
