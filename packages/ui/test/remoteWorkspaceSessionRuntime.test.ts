import assert from "node:assert/strict";
import test from "node:test";

import { markRemoteWorkspaceRunningTasksFailed } from "../src/lib/remoteWorkspaceSessionRuntime.js";
import type { WorkspaceModeUIState } from "../src/store/modeSessionStore.js";

// 远端连接关闭后的 tab 降级：只收口本地 UI runtime（不写远端 snapshot），
// 让任务列表不再永久停在 loading；重连后仍以远端持久状态为准。

function workspaceState(overrides: Partial<WorkspaceModeUIState> = {}): WorkspaceModeUIState {
  return {
    activeTaskId: undefined,
    draftRuntime: { status: "idle", error: null },
    taskRuntimeByTaskId: {},
    taskListCache: [],
    optimisticTaskListByTaskId: {},
    ...overrides,
  } as unknown as WorkspaceModeUIState;
}

type MarkCall = {
  workspacePath: string;
  taskId: string;
  status: string;
  reason: string;
  workspaceIdentity?: string;
};

function harness(states: Record<string, WorkspaceModeUIState>) {
  const calls: MarkCall[] = [];
  return {
    calls,
    getWorkspaceState: (workspacePath: string, workspaceIdentity?: string) =>
      states[workspaceIdentity?.trim() || workspacePath] ?? workspaceState(),
    setTaskRuntimeState: (
      workspacePath: string,
      taskId: string,
      status: string,
      reason: string,
      workspaceIdentity?: string,
    ) => {
      calls.push({ workspacePath, taskId, status, reason, workspaceIdentity });
    },
  };
}

test("活跃任务与 runtime running 去重后只收口一次，并带上断连原因", () => {
  const { calls, ...api } = harness({
    "/w": workspaceState({
      activeTaskId: "task-active",
      taskRuntimeByTaskId: {
        "task-active": { status: "streaming", error: null } as never,
        "task-done": { status: "completed", error: null } as never,
      },
      taskListCache: [
        { taskId: "task-active", status: "running" },
        { taskId: "task-done", status: "running" },
      ] as never,
    }),
  });

  const count = markRemoteWorkspaceRunningTasksFailed({
    tabs: [{ workspacePath: "/w" }],
    getWorkspaceState: api.getWorkspaceState,
    setTaskRuntimeState: api.setTaskRuntimeState,
    reason: "远程连接已断开",
  });

  assert.equal(count, 1, "同一任务在 active + runtime + meta 里出现多次，只收口一次");
  assert.deepEqual(calls, [
    {
      workspacePath: "/w",
      taskId: "task-active",
      status: "failed",
      reason: "远程连接已断开",
      workspaceIdentity: undefined,
    },
  ]);
});

test("本地 runtime 已是终态时，滞后的 meta running 不得被改回 failed", () => {
  const { calls, ...api } = harness({
    "/w": workspaceState({
      taskRuntimeByTaskId: {
        "task-a": { status: "completed", error: null } as never,
        "task-b": { status: "failed", error: "boom" } as never,
      },
      taskListCache: [
        { taskId: "task-a", status: "running" },
        { taskId: "task-b", status: "running" },
      ] as never,
    }),
  });

  const count = markRemoteWorkspaceRunningTasksFailed({
    tabs: [{ workspacePath: "/w" }],
    getWorkspaceState: api.getWorkspaceState,
    setTaskRuntimeState: api.setTaskRuntimeState,
    reason: "remote closed",
  });

  assert.equal(count, 0);
  assert.deepEqual(calls, []);
});

test("optimistic running 也参与收口；空列表返回 0", () => {
  const { calls, ...api } = harness({
    "/w": workspaceState({
      optimisticTaskListByTaskId: {
        "task-opt": { taskId: "task-opt", status: "running" },
      } as never,
    }),
    "/empty": workspaceState(),
  });

  assert.equal(
    markRemoteWorkspaceRunningTasksFailed({
      tabs: [{ workspacePath: "/w" }, { workspacePath: "/empty" }],
      getWorkspaceState: api.getWorkspaceState,
      setTaskRuntimeState: api.setTaskRuntimeState,
      reason: "closed",
    }),
    1,
  );
  assert.deepEqual(
    calls.map((call) => call.taskId),
    ["task-opt"],
  );
});

test("同一 identity 的多个 tab 去重；不同 workspace 的同名 taskId 分别收口", () => {
  const identity = "remote:lan:127.0.0.1:45880:/w";
  const { calls, ...api } = harness({
    [identity]: workspaceState({
      taskRuntimeByTaskId: { shared: { status: "streaming", error: null } as never },
    }),
    "/other": workspaceState({
      taskRuntimeByTaskId: { shared: { status: "creating", error: null } as never },
    }),
  });

  const count = markRemoteWorkspaceRunningTasksFailed({
    tabs: [
      { workspacePath: "/w", workspaceIdentity: identity },
      // 同一 workspace 的第二个 tab：identity 相同，必须去重。
      { workspacePath: "/w", workspaceIdentity: identity },
      { workspacePath: "/other" },
    ],
    getWorkspaceState: api.getWorkspaceState,
    setTaskRuntimeState: api.setTaskRuntimeState,
    reason: "closed",
  });

  assert.equal(count, 2, "identity 相同的重复 tab 去重；另一个 workspace 的 taskId 独立计数");
  assert.deepEqual(calls, [
    {
      workspacePath: "/w",
      taskId: "shared",
      status: "failed",
      reason: "closed",
      workspaceIdentity: identity,
    },
    {
      workspacePath: "/other",
      taskId: "shared",
      status: "failed",
      reason: "closed",
      workspaceIdentity: undefined,
    },
  ]);
});
