import assert from "node:assert/strict";
import test from "node:test";

import { modeSessionEventSchema } from "../src/mode-protocol/index.js";

// spec: docs/plans/2026-10-09-tauri-rewrite-roadmap.md（M4a stream-latency Host 段顺带发现）。
//
// 2026-10-24 回归背景：CLI 的 TurnStarted payload 带 executionStartedAt
// （performance.timeOrigin + performance.now() 的浮点 epoch 毫秒），Host 侧
// modeTurnStartedEventPayloadSchema 是 strict 且未声明该键 → unrecognized_keys
// → modeAgentService「丢弃无效 Mode Protocol session event」整条丢弃。
// stream-latency-host bench 的实测输出里每次 turn 都可见这条丢弃日志。
// 本用例钉住：带该字段（含浮点）的 turn.started 必须能通过信封校验。

function turnStartedEnvelope(payload: Record<string, unknown>): Record<string, unknown> {
  return {
    eventId: "evt-bench-0001",
    sessionId: "sess_bench-0001",
    seq: 1,
    timestamp: 1_761_260_000_000,
    type: "turn.started",
    payload,
  };
}

test("turn.started 信封接受 executionStartedAt（浮点 epoch 毫秒）", () => {
  const parsed = modeSessionEventSchema.parse(
    turnStartedEnvelope({
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
    turnStartedEnvelope({ turnNumber: 2, input: "不带 executionStartedAt 的旧形状" }),
  );
  assert.equal(parsed.type, "turn.started");
});
