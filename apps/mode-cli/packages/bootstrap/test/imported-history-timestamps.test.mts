/**
 * 导入历史的时间线：createdAt / updatedAt 决定导入会话的 `time.created` / `time.updated`，
 * 进而决定 tasks-index 的 `updatedAt`——它同时是远程工作区对话记录镜像「已同步到哪一版」的水位
 * （docs/specs/remote-workspace-conversation-mirror.md）。
 *
 * 曾经 remoteMirror 走的是「非 claudeCode ⇒ updatedAt = createdAt」分支：镜像会话的 updatedAt
 * 恒等于创建时刻，于是①两端排序 / 时间戳不一致，②每轮同步都判定「来源更新」而重导一遍。
 *
 * 运行：pnpm --filter @mode/bootstrap test
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { resolveImportedHistoryTimestamps } from "../src/mode-protocol/server-operations.js";
import type { ModeSessionCreateParams } from "@mode/shared";

type ImportedHistory = NonNullable<ModeSessionCreateParams["importedHistory"]>;

const NOW = 1_700_000_000_000;

function remoteMirror(overrides: Record<string, unknown>): ImportedHistory {
  return {
    source: "remoteMirror",
    title: "镜像会话",
    originSessionId: "sess-origin",
    messages: [
      { role: "user", content: "第一句", timestamp: 1_000 },
      { role: "assistant", content: "第二句", timestamp: 2_000 },
    ],
    ...overrides,
  } as unknown as ImportedHistory;
}

test("remoteMirror 透传 createdAt / updatedAt：水位不会被压回创建时刻", () => {
  const { createdAt, updatedAt } = resolveImportedHistoryTimestamps({
    importedHistory: remoteMirror({ createdAt: 1_000, updatedAt: 9_000 }),
    now: NOW,
  });

  assert.equal(createdAt, 1_000);
  assert.equal(updatedAt, 9_000);
});

test("remoteMirror 缺 updatedAt 时用最后一条消息时间兜底（不是 createdAt）", () => {
  const { createdAt, updatedAt } = resolveImportedHistoryTimestamps({
    importedHistory: remoteMirror({ createdAt: 1_000 }),
    now: NOW,
  });

  assert.equal(createdAt, 1_000);
  assert.equal(updatedAt, 2_000);
});

test("remoteMirror 两个时间都缺时用首/末条消息时间，再退回 now", () => {
  const withMessages = resolveImportedHistoryTimestamps({
    importedHistory: remoteMirror({}),
    now: NOW,
  });
  assert.equal(withMessages.createdAt, 1_000);
  assert.equal(withMessages.updatedAt, 2_000);

  const withoutTimestamps = resolveImportedHistoryTimestamps({
    importedHistory: remoteMirror({
      messages: [
        { role: "user", content: "第一句" },
        { role: "assistant", content: "第二句" },
      ],
    }),
    now: NOW,
  });
  assert.equal(withoutTimestamps.createdAt, NOW);
  assert.equal(withoutTimestamps.updatedAt, NOW);
});

test("claudeCode 行为不变：同样透传 updatedAt 并用消息时间兜底", () => {
  const history = {
    source: "claudeCode",
    title: "Claude 导入",
    createdAt: 5_000,
    updatedAt: 7_000,
    messages: [{ role: "user", content: "hi", timestamp: 6_000 }],
  } as unknown as ImportedHistory;

  assert.deepEqual(resolveImportedHistoryTimestamps({ importedHistory: history, now: NOW }), {
    createdAt: 5_000,
    updatedAt: 7_000,
  });
});

test("没有真实时间线的来源（sharedContext）只有一个时刻：updatedAt 退化为 createdAt", () => {
  const history = {
    source: "sharedContext",
    title: "分享上下文",
    createdAt: 3_000,
    messages: [{ role: "user", content: "ctx", timestamp: 9_999 }],
  } as unknown as ImportedHistory;

  assert.deepEqual(resolveImportedHistoryTimestamps({ importedHistory: history, now: NOW }), {
    createdAt: 3_000,
    updatedAt: 3_000,
  });
});
