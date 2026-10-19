import assert from "node:assert/strict";
import test from "node:test";

import { SessionEventType } from "@mode/contracts";

import { changesSubagentDirectory, isSubagentToolMirror } from "../src/app-subagent-events.js";

function event(type: string, payload: Record<string, unknown> = {}) {
  return { type, payload } as never;
}

test("子代理镜像：只有工具事件且 source=subagent 才算镜像", () => {
  assert.equal(
    isSubagentToolMirror(event(SessionEventType.ToolCallScheduled, { source: "subagent" })),
    true,
  );
  assert.equal(
    isSubagentToolMirror(event(SessionEventType.ToolCallResult, { source: "subagent" })),
    true,
  );
  assert.equal(isSubagentToolMirror(event(SessionEventType.ToolCallScheduled)), false);
  assert.equal(
    isSubagentToolMirror(event(SessionEventType.TurnStarted, { source: "subagent" })),
    false,
  );
});

test("子代理目录刷新：生命周期事件触发，镜像事件与逐 token 事件不触发", () => {
  assert.equal(changesSubagentDirectory(event(SessionEventType.SubagentSpawned)), true);
  assert.equal(changesSubagentDirectory(event(SessionEventType.TurnStarted)), true);
  assert.equal(changesSubagentDirectory(event(SessionEventType.TurnComplete)), true);
  assert.equal(
    changesSubagentDirectory(event(SessionEventType.ToolCallResult, { source: "subagent" })),
    false,
  );
  assert.equal(changesSubagentDirectory(event(SessionEventType.ModelStreaming)), false);
});
