import assert from "node:assert/strict";
import test from "node:test";

import {
  EMPTY_TUI_WORKFLOW_MIRROR,
  buildTuiWorkflowCardIndex,
  seedWorkflowMirror,
  workflowRunStepCounts,
} from "../src/app-workflow-mirror.js";

const node = (phase: string) => ({ siteId: `s-${phase}`, ordinal: 0, phase });

test("工作流步数：节点数加未入表数，settled 按 phase 与未入表结算数", () => {
  assert.deepEqual(
    workflowRunStepCounts({
      nodes: [node("settled"), node("executing")],
      usage: { spentTokens: 0, nodesUsed: 2 },
    } as never),
    { nodesSettled: 1, nodesTotal: 2 },
  );

  assert.deepEqual(
    workflowRunStepCounts({
      nodes: [node("settled"), node("executing")],
      usage: {
        spentTokens: 10,
        nodesUsed: 2,
        nodesUnlisted: 1,
        nodesUnlistedSettled: 1,
      },
    } as never),
    { nodesSettled: 2, nodesTotal: 3 },
  );
});

test("工作流卡片索引：按 toolCallId 建表，无关联键的 run 不进表", () => {
  const mirror = {
    state: {
      revision: 2,
      runs: [
        {
          runId: "r1",
          toolCallId: "t1",
          status: "completed",
          usage: { spentTokens: 1, nodesUsed: 0 },
          nodes: [],
        },
        {
          runId: "r2",
          status: "running",
          usage: { spentTokens: 0, nodesUsed: 0 },
          nodes: [],
        },
      ],
    },
    logTailByRunId: { r1: ["line1", "line2"] },
    seedByRunId: {},
  };
  const index = buildTuiWorkflowCardIndex(mirror as never);
  assert.equal(index.size, 1);
  assert.deepEqual(index.get("t1"), {
    runId: "r1",
    status: "completed",
    nodesSettled: 0,
    nodesTotal: 0,
    usage: { spentTokens: 1, nodesUsed: 0 },
    actors: undefined,
    logTail: ["line1", "line2"],
  });
  assert.deepEqual(buildTuiWorkflowCardIndex(EMPTY_TUI_WORKFLOW_MIRROR as never).size, 0);
});

test("工作流卡片索引：无日志时 logTail 为空数组", () => {
  const index = buildTuiWorkflowCardIndex({
    state: {
      revision: 1,
      runs: [
        {
          runId: "r1",
          toolCallId: "t1",
          status: "running",
          usage: { spentTokens: 0, nodesUsed: 0 },
          nodes: [],
        },
      ],
    },
    logTailByRunId: {},
    seedByRunId: {},
  } as never);
  assert.deepEqual(index.get("t1")?.logTail, []);
});

test("工作流种子：逐个并入 seedByRunId，已有 runId 不被覆盖", () => {
  const once = seedWorkflowMirror(
    EMPTY_TUI_WORKFLOW_MIRROR as never,
    [
      { runId: "r1", toolCallId: "t1" },
      { runId: "r2", toolCallId: "t2" },
    ] as never,
  );
  assert.deepEqual(once.seedByRunId, {
    r1: { runId: "r1", toolCallId: "t1" },
    r2: { runId: "r2", toolCallId: "t2" },
  });
  assert.deepEqual(once.state, { revision: 0, runs: [] });

  const again = seedWorkflowMirror(once as never, [{ runId: "r1", toolCallId: "tX" }] as never);
  assert.deepEqual(again.seedByRunId.r1, { runId: "r1", toolCallId: "t1" });
});
