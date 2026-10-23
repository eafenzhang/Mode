/**
 * window-controller 服务端（docs/specs/window-host-controller-service.md）的单测。
 *
 * 覆盖三层：
 * 1. 聚合运行时（`createWindowHostControllerRuntime`）：三分区 membership 过滤、scope 路由、
 *    排序/分页、mutateTask 写路由、断连冻结与摘除、source 读取失败的按 source 隔离；
 * 2. 远程服务接线（`createRemoteWorkspaceHostService`）：`resolveControllerRemoteSource`
 *    的 online/offline/null 三态与 disconnect/dispose/rebind 生命周期钩子；
 * 3. 装配静态守卫：`serviceAccessor.ts` 必须把 `IWindowControllerService` 注册进本地集合——
 *    漏掉这一行，侧栏 sessions-index 视图会对着未注册频道整体空掉（d71ed992 回归的形状）。
 */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

import { VSBuffer, createFrameDemultiplexer, type IMessagePassingProtocol } from "@mode/rpc";
import type { ModeTaskMeta } from "@mode/shared";
import {
  CONTROLLER_TASKS_INDEX_TOPIC,
  type WindowHostControllerTaskFrame,
} from "@mode/shared/mode-protocol-v4";
import {
  createServiceDescriptor,
  ServiceCollection,
  type IModeTaskService,
  type ModeTaskListQuery,
} from "@mode/services";

import { createWindowHostControllerRuntime } from "../src/host-standalone/windowController/windowHostControllerService.js";
import {
  createRemoteWorkspaceHostService,
  type ControllerSourceLifecycleScope,
} from "../src/host-standalone/remoteWorkspaceService.js";
import type { WindowRemoteConnectionHandle } from "../src/host/windowRemoteConnectionRegistry.js";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "../../..");

const delay = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

let nextMetaSeq = 0;
function meta(overrides: Partial<ModeTaskMeta> & { title: string }): ModeTaskMeta {
  nextMetaSeq += 1;
  return {
    taskId: `task-${nextMetaSeq}`,
    traceId: `trace-${nextMetaSeq}`,
    workspacePath: "D:\\w",
    createdAt: 1_000 + nextMetaSeq,
    updatedAt: 2_000 + nextMetaSeq,
    mode: "build",
    ...overrides,
  } as ModeTaskMeta;
}

/** runtime 只触达 IModeTaskService 的列表/写/事件面；用最小假体承载。 */
function createFakeTaskService(initial: {
  active: ModeTaskMeta[];
  pinned: ModeTaskMeta[];
  archived: ModeTaskMeta[];
}) {
  const state = {
    active: [...initial.active],
    pinned: [...initial.pinned],
    archived: [...initial.archived],
  };
  const calls: string[] = [];
  let workspaceListener: (() => void) | null = null;
  const service = {
    async listTasks() {
      calls.push("listTasks");
      return [...state.active];
    },
    async listPinnedTasks() {
      calls.push("listPinnedTasks");
      return [...state.pinned];
    },
    async listArchivedTasks() {
      calls.push("listArchivedTasks");
      return [...state.archived];
    },
    async setTaskPinned(params: { taskId: string; pinned: boolean }) {
      calls.push(`setTaskPinned:${params.taskId}:${params.pinned}`);
      const found =
        state.active.find((task) => task.taskId === params.taskId) ??
        state.pinned.find((task) => task.taskId === params.taskId);
      if (!found) throw new Error("task not found");
      state.active = state.active.filter((task) => task.taskId !== params.taskId);
      state.pinned = state.pinned.filter((task) => task.taskId !== params.taskId);
      if (params.pinned) state.pinned.push(found);
      else state.active.push(found);
      return { ...found };
    },
    async archiveTask(params: { taskId: string }) {
      calls.push(`archiveTask:${params.taskId}`);
      const found =
        state.active.find((task) => task.taskId === params.taskId) ??
        state.pinned.find((task) => task.taskId === params.taskId);
      if (!found) throw new Error("task not found");
      state.active = state.active.filter((task) => task.taskId !== params.taskId);
      state.pinned = state.pinned.filter((task) => task.taskId !== params.taskId);
      state.archived.push(found);
      return { ...found };
    },
    async unarchiveTask(params: { taskId: string }) {
      calls.push(`unarchiveTask:${params.taskId}`);
      const found = state.archived.find((task) => task.taskId === params.taskId);
      if (!found) throw new Error("task not found");
      state.archived = state.archived.filter((task) => task.taskId !== params.taskId);
      state.active.push(found);
      return { ...found };
    },
    async deleteTask(params: { taskId: string }) {
      calls.push(`deleteTask:${params.taskId}`);
      state.active = state.active.filter((task) => task.taskId !== params.taskId);
      state.pinned = state.pinned.filter((task) => task.taskId !== params.taskId);
    },
    async deleteArchivedTask(params: { taskId: string }) {
      calls.push(`deleteArchivedTask:${params.taskId}`);
      state.archived = state.archived.filter((task) => task.taskId !== params.taskId);
      return true;
    },
    async deleteArchivedTasks(params: { taskIds: string[] }) {
      calls.push(`deleteArchivedTasks:${params.taskIds.join(",")}`);
      const deletedTaskIds = params.taskIds.filter((taskId) =>
        state.archived.some((task) => task.taskId === taskId),
      );
      state.archived = state.archived.filter((task) => !params.taskIds.includes(task.taskId));
      return { deletedTaskIds, skippedTaskIds: [] as string[], failedTaskIds: [] as string[] };
    },
    async setTaskUnread(params: { taskId: string; unread: boolean }) {
      calls.push(`setTaskUnread:${params.taskId}:${params.unread}`);
      const found = [...state.active, ...state.pinned, ...state.archived].find(
        (task) => task.taskId === params.taskId,
      );
      if (!found) throw new Error("task not found");
      return { ...found, unreadAt: params.unread ? Date.now() : undefined };
    },
    onDynamicWorkspaceEvent() {
      return (listener: () => void) => {
        workspaceListener = listener;
        return {
          dispose: () => {
            workspaceListener = null;
          },
        };
      };
    },
    emitWorkspaceEvent() {
      workspaceListener?.();
    },
  };
  return { service: service as unknown as IModeTaskService, state, calls };
}

function activeQuery(overrides: Partial<ModeTaskListQuery> = {}): ModeTaskListQuery {
  return {
    kind: "active",
    workspaceScopes: [{ workspacePath: "D:\\w" }],
    sortBy: "updated",
    ...overrides,
  };
}

test("listTaskList 聚合本地 source 三分区并按 kind 过滤 membership", async () => {
  const active1 = meta({ title: "active-1" });
  const active2 = meta({ title: "active-2" });
  const pinned1 = meta({ title: "pinned-1" });
  const archived1 = meta({ title: "archived-1" });
  const fake = createFakeTaskService({
    active: [active1, active2],
    pinned: [pinned1],
    archived: [archived1],
  });
  const runtime = createWindowHostControllerRuntime({
    createId: (() => {
      let next = 0;
      return () => `id-${++next}`;
    })(),
    resolveSource: () => ({
      scope: { kind: "local", workspacePath: "D:\\w" },
      taskService: fake.service,
      sourceAvailability: "online",
    }),
  });

  const active = await runtime.service.listTaskList(activeQuery());
  // kind "active" 的 membership 口径 = !archived（pinned 行仍属于 active 分区）。
  assert.deepEqual(active.items.map((item) => item.title).sort(), [
    "active-1",
    "active-2",
    "pinned-1",
  ]);
  assert.ok(active.items.every((item) => item.sourceAvailability === "online"));

  const pinned = await runtime.service.listTaskList(activeQuery({ kind: "pinned" }));
  assert.deepEqual(
    pinned.items.map((item) => item.title),
    ["pinned-1"],
  );

  const archived = await runtime.service.listTaskList(activeQuery({ kind: "archived" }));
  assert.deepEqual(
    archived.items.map((item) => item.title),
    ["archived-1"],
  );

  // timeline 与 active 同口径（!pinned && !archived）。
  const timeline = await runtime.service.listTaskList(activeQuery({ kind: "timeline" }));
  assert.equal(timeline.items.length, 2);

  // meta 被归一到 source scope：workspacePath/identity 与投影一致。
  assert.ok(active.items.every((item) => item.workspacePath === "D:\\w"));
  runtime.dispose();
});

test("listTaskList 排序与分页 limit/hasMore", async () => {
  const older = meta({ title: "older", updatedAt: 100 });
  const newer = meta({ title: "newer", updatedAt: 200 });
  const fake = createFakeTaskService({ active: [older, newer], pinned: [], archived: [] });
  const runtime = createWindowHostControllerRuntime({
    createId: () => "id",
    resolveSource: () => ({
      scope: { kind: "local", workspacePath: "D:\\w" },
      taskService: fake.service,
      sourceAvailability: "online",
    }),
  });

  const sorted = await runtime.service.listTaskList(activeQuery({ sortBy: "updated" }));
  assert.deepEqual(
    sorted.items.map((item) => item.title),
    ["newer", "older"],
  );

  const byCreated = await runtime.service.listTaskList(activeQuery({ sortBy: "created" }));
  assert.deepEqual(
    byCreated.items.map((item) => item.title),
    ["newer", "older"],
  );

  const paged = await runtime.service.listTaskList(activeQuery({ limit: 1 }));
  assert.equal(paged.total, 2);
  assert.equal(paged.items.length, 1);
  assert.equal(paged.hasMore, true);
  runtime.dispose();
});

test("remote scope 经 resolveSource 路由，行携带 remoteSessionId 与 identity", async () => {
  const task = meta({ title: "remote-task" });
  const fake = createFakeTaskService({ active: [task], pinned: [], archived: [] });
  const identity = "remote:lan:100.90.96.50:45880:D:/w";
  const runtime = createWindowHostControllerRuntime({
    createId: () => "id",
    resolveSource: (scope) =>
      scope.workspaceIdentity === identity
        ? {
            scope: {
              kind: "remote",
              remoteSessionId: "sess-1",
              workspacePath: "D:/w",
              workspaceIdentity: identity,
            },
            taskService: fake.service,
            sourceAvailability: "online",
          }
        : null,
  });

  const result = await runtime.service.listTaskList(
    activeQuery({
      workspaceScopes: [{ workspacePath: "D:/w", workspaceIdentity: identity }],
    }),
  );
  assert.equal(result.items.length, 1);
  assert.equal(result.items[0]?.remoteSessionId, "sess-1");
  assert.equal(result.items[0]?.workspaceIdentity, identity);
  runtime.dispose();
});

test("远程身份 scope 无匹配 source 时不回落本地 tasks-index（fail-closed）", async () => {
  const localTask = meta({ title: "local-task" });
  const fake = createFakeTaskService({ active: [localTask], pinned: [], archived: [] });
  const identity = "remote:lan:other-host:45880:D:/w";
  const runtime = createWindowHostControllerRuntime({
    createId: () => "id",
    // 镜像 serviceAccessor 的装配口径：远程身份查不到会话时 resolveSource 返回 null。
    resolveSource: (scope) => {
      if (scope.workspaceIdentity === identity) return null;
      return {
        scope: { kind: "local", workspacePath: "D:\\w" },
        taskService: fake.service,
        sourceAvailability: "online",
      };
    },
  });

  const result = await runtime.service.listTaskList(
    activeQuery({
      workspaceScopes: [{ workspacePath: "D:/w", workspaceIdentity: identity }],
    }),
  );
  assert.equal(result.items.length, 0);
  assert.equal(fake.calls.includes("listTasks"), false);
  runtime.dispose();
});

test("mutateTask 走 source 写路由并回写投影；订阅者收到差量帧", async () => {
  const keep = meta({ title: "keep" });
  const target = meta({ title: "target" });
  const fake = createFakeTaskService({ active: [keep, target], pinned: [], archived: [] });
  const runtime = createWindowHostControllerRuntime({
    createId: (() => {
      let next = 0;
      return () => `id-${++next}`;
    })(),
    resolveSource: () => ({
      scope: { kind: "local", workspacePath: "D:\\w" },
      taskService: fake.service,
      sourceAvailability: "online",
    }),
  });

  const initial = await runtime.service.listTaskList(activeQuery());
  assert.equal(initial.items.length, 2);

  const frames: WindowHostControllerTaskFrame[] = [];
  // 监听器必须先于订阅注册：subscribe 的 snapshot 帧在 ACK 内同步发出。
  runtime.service.onDynamicControllerFrame()((frame) => {
    if (frame.topic === CONTROLLER_TASKS_INDEX_TOPIC) {
      frames.push(frame as WindowHostControllerTaskFrame);
    }
  });
  const subscription = await runtime.service.subscribeControllerV4({
    topic: CONTROLLER_TASKS_INDEX_TOPIC,
    visibility: "foreground",
  });

  await runtime.service.mutateTask({
    address: {
      taskId: target.taskId,
      workspacePath: "D:\\w",
      workspaceIdentity: undefined,
      remoteSessionId: undefined,
    },
    mutation: { kind: "archive", archived: true },
  });
  assert.ok(fake.calls.includes(`archiveTask:${target.taskId}`));

  await delay(20);
  const afterMutation = await runtime.service.listTaskList(activeQuery());
  assert.deepEqual(
    afterMutation.items.map((item) => item.title),
    ["keep"],
  );

  // snapshot（订阅即发）+ 至少一条 deltas 帧（archive 后的 membership 变更）。
  assert.ok(frames.length >= 2);
  assert.equal(frames[0]?.payload.kind, "snapshot");
  assert.ok(
    frames.slice(1).some((frame) => frame.payload.kind === "deltas"),
    "archive 后应发布 task deltas 帧",
  );

  await runtime.service.unsubscribeControllerV4({
    subscriptionId: subscription.ack.subscriptionId,
  });
  runtime.dispose();
});

test("workspace 事件驱动 source 刷新，列表随后收敛", async () => {
  const task = meta({ title: "task" });
  const fake = createFakeTaskService({ active: [task], pinned: [], archived: [] });
  const runtime = createWindowHostControllerRuntime({
    createId: () => "id",
    resolveSource: () => ({
      scope: { kind: "local", workspacePath: "D:\\w" },
      taskService: fake.service,
      sourceAvailability: "online",
    }),
  });

  await runtime.service.listTaskList(activeQuery());
  // 模拟 base modeTaskService 的 workspace_task_list_changed：状态变更后发事件。
  fake.state.active = [];
  fake.state.archived.push({ ...task });
  (fake.service as unknown as { emitWorkspaceEvent(): void }).emitWorkspaceEvent();
  await delay(20);

  const after = await runtime.service.listTaskList(activeQuery());
  assert.equal(after.items.length, 0);
  runtime.dispose();
});

test("source 离线时冻结行以 offline 展示且拒绝写操作；removeSource 摘除", async () => {
  const task = meta({ title: "task" });
  const fake = createFakeTaskService({ active: [task], pinned: [], archived: [] });
  let availability: "online" | "offline" = "online";
  const runtime = createWindowHostControllerRuntime({
    createId: () => "id",
    // 镜像真实装配：连接断开时 registry 与 Controller 同步进入 offline。
    resolveSource: () => ({
      scope: { kind: "local", workspacePath: "D:\\w" },
      taskService: availability === "online" ? fake.service : undefined,
      sourceAvailability: availability,
    }),
  });

  await runtime.service.listTaskList(activeQuery());
  availability = "offline";
  runtime.disconnectSource({ kind: "local", workspacePath: "D:\\w" });

  const offline = await runtime.service.listTaskList(activeQuery());
  assert.equal(offline.items.length, 1);
  assert.equal(offline.items[0]?.sourceAvailability, "offline");

  await assert.rejects(
    runtime.service.mutateTask({
      address: {
        taskId: task.taskId,
        workspacePath: "D:\\w",
        workspaceIdentity: undefined,
        remoteSessionId: undefined,
      },
      mutation: { kind: "pin", pinned: true },
    }),
    /本地 source 当前不可用/u,
  );

  runtime.removeSource({ kind: "local", workspacePath: "D:\\w" });
  const removed = await runtime.service.listTaskList(activeQuery());
  assert.equal(removed.items.length, 0);
  runtime.dispose();
});

test("source 读取失败只影响该 source（onSourceError 记账），其他 source 不受影响", async () => {
  const good = createFakeTaskService({
    active: [meta({ title: "good" })],
    pinned: [],
    archived: [],
  });
  const broken = createFakeTaskService({ active: [], pinned: [], archived: [] });
  (broken.service as unknown as { listTasks: () => Promise<never> }).listTasks = async () => {
    throw new Error("index read failed");
  };
  const errors: string[] = [];
  const runtime = createWindowHostControllerRuntime({
    createId: () => "id",
    resolveSource: (scope) => ({
      scope: { kind: "local", workspacePath: scope.workspacePath },
      taskService: scope.workspacePath === "D:\\broken" ? broken.service : good.service,
      sourceAvailability: "online",
    }),
    onSourceError: (scope, operation, error) => {
      errors.push(`${operation}:${scope.workspacePath}:${String(error)}`);
    },
  });

  const result = await runtime.service.listTaskList(
    activeQuery({
      workspaceScopes: [{ workspacePath: "D:\\broken" }, { workspacePath: "D:\\w" }],
    }),
  );
  assert.deepEqual(
    result.items.map((item) => item.title),
    ["good"],
  );
  assert.ok(errors.some((entry) => entry.startsWith("refresh:D:\\broken")));
  runtime.dispose();
});

// ---------------------------------------------------------------------------
// 远程服务接线：resolveControllerRemoteSource 三态与生命周期钩子。
// ---------------------------------------------------------------------------

function transportPair(): { server: IMessagePassingProtocol; client: IMessagePassingProtocol } {
  const aListeners = new Set<(buffer: VSBuffer) => void>();
  const bListeners = new Set<(buffer: VSBuffer) => void>();
  const server: IMessagePassingProtocol = {
    send: (buffer) => {
      for (const listener of aListeners) listener(buffer);
    },
    onMessage: (listener) => {
      bListeners.add(listener);
      return { dispose: () => bListeners.delete(listener) };
    },
  };
  const client: IMessagePassingProtocol = {
    send: (buffer) => {
      for (const listener of bListeners) listener(buffer);
    },
    onMessage: (listener) => {
      aListeners.add(listener);
      return { dispose: () => aListeners.delete(listener) };
    },
  };
  return { server, client };
}

interface EchoService {
  ping(): Promise<string>;
}
const ControllerProbe = createServiceDescriptor<EchoService>("controller-probe");

test("resolveControllerRemoteSource 三态：online / 断连 offline / 未匹配 null", async () => {
  const transport = transportPair();
  const demux = createFrameDemultiplexer(transport.server);
  const { createScopedAttachmentRegistry } =
    await import("../src/host-standalone/scopedAttachments.js");
  const scopedAttachments = createScopedAttachmentRegistry({ demux, log: () => undefined });
  const closeState: {
    listener: ((event: { exitCode: number | null; signal: string | null }) => void) | null;
  } = { listener: null };
  const remoteServices = new ServiceCollection();
  remoteServices.register(ControllerProbe, {
    async ping() {
      return "pong";
    },
  });
  const handle: WindowRemoteConnectionHandle<ServiceCollection, never> = {
    services: remoteServices,
    onDidClose(listener) {
      closeState.listener = listener as never;
      return { dispose: () => undefined };
    },
    async dispose() {},
  };
  const lifecycle = { disconnected: [], disposed: [] } as {
    disconnected: ControllerSourceLifecycleScope[];
    disposed: ControllerSourceLifecycleScope[];
  };
  const adapter = await createRemoteWorkspaceHostService({
    services: new ServiceCollection(),
    log: () => undefined,
    scopedAttachments,
    guardChannel: (_channelName, channel) => channel,
    connectLan: async () => handle,
    createId: (() => {
      let next = 0;
      return () => `id-${++next}`;
    })(),
    onControllerSourceDisconnected: (scope) => {
      lifecycle.disconnected.push(scope);
    },
    onControllerSourceDisposed: (scope) => {
      lifecycle.disposed.push(scope);
    },
  });
  try {
    const scope = {
      workspacePath: "/w",
      workspaceIdentity: "remote:lan:127.0.0.1:45880:/w",
    };
    const connected = await adapter.connect({
      target: { kind: "lan", host: "127.0.0.1", port: 45880 },
      requestId: "request-1",
      workspacePath: scope.workspacePath,
      workspaceIdentity: scope.workspaceIdentity,
    });
    assert.ok(connected.ok);
    const sessionId = connected.ok ? connected.descriptor.remoteSessionId : "";

    const online = adapter.resolveControllerRemoteSource(scope);
    assert.ok(online && online.sourceAvailability === "online");
    assert.equal(online.remoteSessionId, sessionId);
    assert.ok(online.services);

    closeState.listener?.({ exitCode: 1, signal: null });
    await delay(10);
    const offline = adapter.resolveControllerRemoteSource(scope);
    assert.ok(offline && offline.sourceAvailability === "offline");
    assert.equal(offline.remoteSessionId, sessionId);
    assert.deepEqual(lifecycle.disconnected, [
      {
        remoteSessionId: sessionId,
        workspacePath: scope.workspacePath,
        workspaceIdentity: scope.workspaceIdentity,
      },
    ]);

    await adapter.dispose({ sessionId });
    assert.equal(adapter.resolveControllerRemoteSource(scope), null);
    assert.deepEqual(lifecycle.disposed, [
      {
        remoteSessionId: sessionId,
        workspacePath: scope.workspacePath,
        workspaceIdentity: scope.workspaceIdentity,
      },
    ]);
    assert.equal(
      adapter.resolveControllerRemoteSource({
        workspacePath: "/never-bound",
        workspaceIdentity: "remote:lan:127.0.0.1:45880:/never-bound",
      }),
      null,
    );

    // Host 内部方法不得进入 ProxyChannel 的 RPC 面。
    assert.equal(Object.keys(adapter).includes("resolveControllerRemoteSource"), false);
  } finally {
    await adapter.disposeHost();
  }
});

test("bindContext 换绑触发 disposed 钩子携带旧 scope", async () => {
  const transport = transportPair();
  const demux = createFrameDemultiplexer(transport.server);
  const { createScopedAttachmentRegistry } =
    await import("../src/host-standalone/scopedAttachments.js");
  const scopedAttachments = createScopedAttachmentRegistry({ demux, log: () => undefined });
  const remoteServices = new ServiceCollection();
  remoteServices.register(ControllerProbe, {
    async ping() {
      return "pong";
    },
  });
  const handle: WindowRemoteConnectionHandle<ServiceCollection, never> = {
    services: remoteServices,
    onDidClose() {
      return { dispose: () => undefined };
    },
    async dispose() {},
  };
  const disposed: ControllerSourceLifecycleScope[] = [];
  const adapter = await createRemoteWorkspaceHostService({
    services: new ServiceCollection(),
    log: () => undefined,
    scopedAttachments,
    guardChannel: (_channelName, channel) => channel,
    connectLan: async () => handle,
    createId: (() => {
      let next = 0;
      return () => `id-${++next}`;
    })(),
    onControllerSourceDisposed: (scope) => {
      disposed.push(scope);
    },
  });
  try {
    const connected = await adapter.connect({
      target: { kind: "lan", host: "127.0.0.1", port: 45880 },
      requestId: "request-1",
      workspacePath: "/w",
      workspaceIdentity: "remote:lan:127.0.0.1:45880:/w",
    });
    assert.ok(connected.ok);
    const sessionId = connected.ok ? connected.descriptor.remoteSessionId : "";

    // 换绑到新路径：旧 scope 必须从 Controller 摘除（否则旧投影以同 identity 滞留）。
    await adapter.bindContext({ sessionId, workspacePath: "/w2" });
    assert.deepEqual(disposed, [
      {
        remoteSessionId: sessionId,
        workspacePath: "/w",
        workspaceIdentity: "remote:lan:127.0.0.1:45880:/w",
      },
    ]);
  } finally {
    await adapter.disposeHost();
  }
});

test("装配静态守卫：serviceAccessor 注册 window-controller 频道与运行时", () => {
  const source = readFileSync(
    resolve(repoRoot, "packages/desktop/src/host-standalone/serviceAccessor.ts"),
    "utf8",
  );
  assert.ok(
    source.includes("createWindowHostControllerRuntime"),
    "serviceAccessor 必须构造 window-controller 运行时",
  );
  assert.ok(
    source.includes("services.register(IWindowControllerService"),
    "serviceAccessor 必须把 IWindowControllerService 注册进本地服务集合",
  );
  const remoteSource = readFileSync(
    resolve(repoRoot, "packages/desktop/src/host-standalone/remoteWorkspaceService.ts"),
    "utf8",
  );
  assert.ok(
    remoteSource.includes('"resolveControllerRemoteSource"'),
    "resolveControllerRemoteSource 必须列入内部成员隐藏清单（不进 RPC 面）",
  );
});
