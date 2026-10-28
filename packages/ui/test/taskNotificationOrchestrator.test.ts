import assert from "node:assert/strict";
import test from "node:test";

import type { SessionSummary } from "@mode/shared/mode-protocol-v4";
import { collectTerminalTaskNotificationPayloads } from "../src/lib/taskNotificationOrchestrator.js";

const formatMessage = ((descriptor: { id: string }, values?: { title?: string }) =>
  values?.title ? `${descriptor.id}:${values.title}` : descriptor.id) as never;

function session(overrides: Partial<SessionSummary> = {}): SessionSummary {
  return {
    sessionId: "s1",
    workspaceId: "w1",
    title: "任务 A",
    phase: "running",
    sessionEnded: false,
    hasBackgroundWork: false,
    lastActivityAt: 1,
    createdAt: 1,
    ...overrides,
  };
}

test("新出现的终态会话在基线之后也要通知（短任务首帧前完成）", () => {
  const payloads = collectTerminalTaskNotificationPayloads({
    previousBySessionId: new Map(),
    sessions: [session({ phase: "completedSuccess" })],
    formatMessage,
    includeNewTerminalSessions: true,
  });

  assert.equal(payloads.length, 1);
  assert.equal(payloads[0]?.taskId, "s1");
  assert.equal(payloads[0]?.status, "completed");
});

test("首次 live 快照基线不把新终态会话当作边沿（避免恢复历史时重放）", () => {
  const payloads = collectTerminalTaskNotificationPayloads({
    previousBySessionId: new Map(),
    sessions: [session({ phase: "completedSuccess" })],
    formatMessage,
    includeNewTerminalSessions: false,
  });

  assert.equal(payloads.length, 0);
});

test("running → completedSuccess 只通知一次", () => {
  const running = session();
  const completed = session({ phase: "completedSuccess" });

  assert.equal(
    collectTerminalTaskNotificationPayloads({
      previousBySessionId: new Map([[running.sessionId, running]]),
      sessions: [completed],
      formatMessage,
      includeNewTerminalSessions: true,
    }).length,
    1,
  );

  // 同一终态再次出现（conflated 快照重复）不重复通知。
  assert.equal(
    collectTerminalTaskNotificationPayloads({
      previousBySessionId: new Map([[completed.sessionId, completed]]),
      sessions: [completed],
      formatMessage,
      includeNewTerminalSessions: true,
    }).length,
    0,
  );
});

test("running → running 不通知", () => {
  const running = session();

  assert.equal(
    collectTerminalTaskNotificationPayloads({
      previousBySessionId: new Map([[running.sessionId, running]]),
      sessions: [running],
      formatMessage,
      includeNewTerminalSessions: true,
    }).length,
    0,
  );
});

test("error 终态同样按失败通知", () => {
  const running = session();
  const failed = session({ phase: "error" });

  const payloads = collectTerminalTaskNotificationPayloads({
    previousBySessionId: new Map([[running.sessionId, running]]),
    sessions: [failed],
    formatMessage,
    includeNewTerminalSessions: true,
  });

  assert.equal(payloads.length, 1);
  assert.equal(payloads[0]?.status, "failed");
});
