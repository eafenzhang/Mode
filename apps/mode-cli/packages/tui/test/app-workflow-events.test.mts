import assert from "node:assert/strict";
import test from "node:test";

import { applyWorkflowProgressEvent } from "../src/app-workflow-events.js";
import {
  EMPTY_TUI_WORKFLOW_MIRROR,
  applyWorkflowProgressToMirror,
} from "../src/app-workflow-mirror.js";

function apply(
  mirror: unknown,
  payload: Record<string, unknown>,
): ReturnType<typeof applyWorkflowProgressToMirror> {
  let result: unknown;
  applyWorkflowProgressEvent(payload, ((updater: unknown) => {
    result = (updater as (current: unknown) => unknown)(mirror);
  }) as never);
  return result as ReturnType<typeof applyWorkflowProgressToMirror>;
}

test("工作流事件：无效 envelope 保持镜像引用不变", () => {
  assert.equal(apply(EMPTY_TUI_WORKFLOW_MIRROR, { payload: {} }), EMPTY_TUI_WORKFLOW_MIRROR);
});

test("工作流事件：run-started 建出运行态条目", () => {
  const mirror = apply(EMPTY_TUI_WORKFLOW_MIRROR, {
    runId: "r1",
    toolCallId: "t1",
    sequence: 1,
    eventType: "run-started",
    payload: {},
  });
  assert.deepEqual(mirror.state, {
    revision: 1,
    runs: [
      {
        runId: "r1",
        toolCallId: "t1",
        status: "running",
        usage: { spentTokens: 0, nodesUsed: 0 },
        actors: [],
        nodes: [],
        lastEventSequence: 1,
      },
    ],
  });
  assert.deepEqual(mirror.logTailByRunId, {});
});

test("工作流事件：同一事件重放保持引用不变", () => {
  const envelope = {
    runId: "r1",
    toolCallId: "t1",
    sequence: 1,
    eventType: "run-started",
    payload: {},
  };
  const first = apply(EMPTY_TUI_WORKFLOW_MIRROR, envelope);
  assert.equal(apply(first, envelope), first);
});

test("工作流事件：log 折叠空白并追加到 run 的日志尾", () => {
  const first = applyWorkflowProgressToMirror(
    EMPTY_TUI_WORKFLOW_MIRROR as never,
    {
      runId: "r1",
      sequence: 2,
      eventType: "log",
      payload: { message: "  hello   world  " },
    } as never,
  );
  assert.deepEqual(first.logTailByRunId, { r1: ["hello world"] });

  const second = applyWorkflowProgressToMirror(
    first as never,
    {
      runId: "r1",
      sequence: 3,
      eventType: "log",
      payload: { message: "second" },
    } as never,
  );
  assert.deepEqual(second.logTailByRunId, { r1: ["hello world", "second"] });

  const blank = applyWorkflowProgressToMirror(
    first as never,
    {
      runId: "r1",
      sequence: 4,
      eventType: "log",
      payload: { message: "   " },
    } as never,
  );
  assert.deepEqual(blank.logTailByRunId, first.logTailByRunId);
});
