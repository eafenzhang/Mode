import assert from "node:assert/strict";
import test from "node:test";

import { modeSessionEventSchema } from "../src/mode-protocol/index.js";

// spec: docs/specs/tauri-agent-runtime.md §6.6（已知缺陷：v1 宿主 schema 落后于线上——
// 两处半边均已于 2026-10-24 修复，本文件是它们的家族回归）。
//
// 回归背景一（turn.started）：CLI 的 TurnStarted payload 带 executionStartedAt
// （performance.timeOrigin + performance.now() 的浮点 epoch 毫秒），Host 侧
// modeTurnStartedEventPayloadSchema 是 strict 且未声明该键 → unrecognized_keys
// → modeAgentService「丢弃无效 Mode Protocol session event」整条丢弃。
//
// 回归背景二（tool.updated/started）：CLI 的 ToolCallStartedPayload 带
// readOnly / sideEffectScope（解析后的副作用能力，事件在 handler 动手前发出），
// Host 侧 started 变体同样 strict 未声明 → 每一次工具开始事件都被丢弃。
//
// 两族都钉成用例：strict schema 任一侧新增键而另一侧未同步，这里先红。

function sessionEnvelope(
  type: string,
  payload: Record<string, unknown>,
): Record<string, unknown> {
  return {
    eventId: "evt-bench-0001",
    sessionId: "sess_bench-0001",
    seq: 1,
    timestamp: 1_761_260_000_000,
    type,
    payload,
  };
}

test("turn.started 信封接受 executionStartedAt（浮点 epoch 毫秒）", () => {
  const parsed = modeSessionEventSchema.parse(
    sessionEnvelope("turn.started", {
      turnNumber: 1,
      input: "第 1 轮 Host 段流式样本",
      // CLI 实测形状：performance.timeOrigin + performance.now()，带小数部分
      // （这里用 .5——在该量级可被 f64 精确表示，lint 对不可表示字面量会红）。
      executionStartedAt: 1_761_260_000_123.5,
    }),
  );
  assert.equal(parsed.type, "turn.started");
  const payload = parsed.payload as { executionStartedAt?: number } | undefined;
  assert.equal(payload?.executionStartedAt, 1_761_260_000_123.5);
});

test("turn.started 信封在缺省该字段时保持兼容（可选键）", () => {
  const parsed = modeSessionEventSchema.parse(
    sessionEnvelope("turn.started", { turnNumber: 2, input: "不带 executionStartedAt 的旧形状" }),
  );
  assert.equal(parsed.type, "turn.started");
});

test("tool.updated/started 信封接受 readOnly 与 sideEffectScope（协议夹具同形）", () => {
  // 载荷形状逐字对齐真实夹具 session-event-tool-updated-started.json 的 frame.payload。
  const parsed = modeSessionEventSchema.parse(
    sessionEnvelope("tool.updated", {
      toolCallId: "toolu_mock_0_0",
      toolName: "Read",
      startedAt: 1_761_260_000_123,
      readOnly: true,
      sideEffectScope: "none",
      kind: "started",
    }),
  );
  assert.equal(parsed.type, "tool.updated");
  const payload = parsed.payload as {
    kind?: string;
    readOnly?: boolean;
    sideEffectScope?: string;
  } | undefined;
  assert.equal(payload?.kind, "started");
  assert.equal(payload?.readOnly, true);
  assert.equal(payload?.sideEffectScope, "none");
});

test("tool.updated/started 信封覆盖 scope 词表边界与缺省兼容", () => {
  // 7 值词表与 CLI 契约 ToolSideEffectScope 同源；用最后一个值钉住词表上界。
  const parsed = modeSessionEventSchema.parse(
    sessionEnvelope("tool.updated", {
      toolCallId: "toolu_mock_0_1",
      startedAt: 1_761_260_000_123,
      kind: "started",
      readOnly: false,
      sideEffectScope: "userInteraction",
    }),
  );
  assert.equal(parsed.type, "tool.updated");
  // 缺省两键的旧形状（早于这两个字段的事件）必须继续可解析。
  const legacy = modeSessionEventSchema.parse(
    sessionEnvelope("tool.updated", {
      toolCallId: "toolu_mock_0_2",
      startedAt: 1_761_260_000_123,
      kind: "started",
    }),
  );
  assert.equal(legacy.type, "tool.updated");
});
