/**
 * M2 Task 5 Step 2（DR-T5-1 = (a)）的 Host 侧验证面：
 * Rust 运行时只透传 `deliveryKind`，**重放归 Host/relay**。这里锁住 main 进程
 * `TaskRealtimeBus` 的三条重放语义，防止以后改 stream/snapshot/queue 时静默退化：
 *
 * 1. 迟到的 Host 若在批次已经 flush 后加入同一 workspace → 收到**同一 seq 流**的重放
 *    （`deliveryPurpose: "observer"`），不重发 owner 直达事件；
 * 2. 批次尚未 flush（没有可重放内容）时接入 → 收到 `task_snapshot_invalidated` /
 *    `stream_mirror_gap`，由 observer 走快照补齐，不伪造 seq；
 * 3. 终态批次 flush 后批次即删除，再接入的 Host 不会拿到过期重放。
 */
import assert from "node:assert/strict";
import test from "node:test";

import { HostMessageTypes, HostResponseTypes, type TaskRealtimeDeliveredEvent } from "@mode/shared";

import { TaskRealtimeBus } from "../src/main/taskRealtimeBus.ts";

const silentLogger = {
  info: () => {},
  warn: () => {},
};

interface PostedMessage {
  type?: string;
  event?: TaskRealtimeDeliveredEvent;
  result?: { acquired?: boolean };
}

class FakeHostChild {
  readonly messages: PostedMessage[] = [];
  private readonly listeners = new Map<string, Set<(payload: unknown) => void>>();

  on(event: string, listener: (payload: unknown) => void): void {
    const set = this.listeners.get(event) ?? new Set();
    set.add(listener);
    this.listeners.set(event, set);
  }

  off(event: string, listener: (payload: unknown) => void): void {
    this.listeners.get(event)?.delete(listener);
  }

  once(event: string, listener: (payload: unknown) => void): void {
    const wrapped = (payload: unknown): void => {
      this.off(event, wrapped);
      listener(payload);
    };
    this.on(event, wrapped);
  }

  postMessage(message: unknown): void {
    this.messages.push(message as PostedMessage);
  }

  emit(event: string, payload?: unknown): void {
    for (const listener of this.listeners.get(event) ?? []) {
      listener(payload);
    }
  }

  deliveries(): TaskRealtimeDeliveredEvent[] {
    return this.messages
      .filter((message) => message.type === HostMessageTypes.TaskRealtimeDeliver)
      .map((message) => message.event as TaskRealtimeDeliveredEvent);
  }
}

const WORKSPACE_PATH = "/tmp/mode-replay-workspace";
const TASK_ID = "task-1";
const RUN_ID = "trace-1";

const target = {
  workspacePath: WORKSPACE_PATH,
  workspaceKey: WORKSPACE_PATH,
  taskId: TASK_ID,
  runId: RUN_ID,
  traceId: RUN_ID,
};

function register(bus: TaskRealtimeBus, hostId: string, child: FakeHostChild): void {
  bus.registerHost({
    hostId,
    windowId: 1,
    child: child as never,
    workspaceKeys: [WORKSPACE_PATH],
  });
}

function acquireLease(bus: TaskRealtimeBus, child: FakeHostChild): void {
  child.emit("message", {
    type: HostResponseTypes.TaskRunLeaseAcquire,
    request: { ...target, leaseRequestId: `lease-${Math.random()}` },
  });
}

function publishUserMessage(child: FakeHostChild, content: string): void {
  child.emit("message", {
    type: HostResponseTypes.TaskStreamOpPublish,
    target,
    op: {
      kind: "user_message",
      messageId: `message-${content}`,
      content,
      timestamp: Date.now(),
    },
  });
}

function publishTerminal(child: FakeHostChild): void {
  child.emit("message", {
    type: HostResponseTypes.TaskStreamOpPublish,
    target,
    op: {
      kind: "stream_event",
      event: { type: "task_complete", taskId: TASK_ID, traceId: RUN_ID },
    },
  });
}

test("flush 之后接入的 Host 收到同 seq 流的 observer 重放", async () => {
  const bus = new TaskRealtimeBus({ logger: silentLogger });
  const owner = new FakeHostChild();
  register(bus, "host-owner", owner);

  acquireLease(bus, owner);
  assert.equal(
    owner.messages.find((message) => message.type === HostMessageTypes.TaskRunLeaseResult)?.result
      ?.acquired,
    true,
  );

  publishUserMessage(owner, "hello");
  // 非终态 op 由 1s 批量定时器 flush。
  await new Promise((resolve) => setTimeout(resolve, 1200));

  const ownerBatches = owner
    .deliveries()
    .filter((event) => event.type === "task_stream_mirror_batch");
  assert.equal(ownerBatches.length, 1);
  assert.equal(ownerBatches[0]?.deliveryPurpose, "relay_owner");
  assert.equal(ownerBatches[0]?.fromSeq, 1);

  const late = new FakeHostChild();
  register(bus, "host-late", late);

  const replayed = late.deliveries();
  assert.equal(replayed.length, 1);
  assert.equal(replayed[0]?.type, "task_stream_mirror_batch");
  assert.equal(replayed[0]?.deliveryPurpose, "observer");
  assert.equal(replayed[0]?.fromSeq, 1);
  assert.equal(replayed[0]?.toSeq, 1);
  assert.equal(replayed[0]?.batchSeq, 1);

  // 重放是幂等的：同一 Host 再次更新 workspace 范围不会重复回放。
  bus.updateHostWorkspaceKeys("host-late", [WORKSPACE_PATH]);
  assert.equal(late.deliveries().length, 1);
});

test("批次尚未 flush 时接入的 Host 收到 stream_mirror_gap 快照失效，而不是断流", () => {
  const bus = new TaskRealtimeBus({ logger: silentLogger });
  const owner = new FakeHostChild();
  register(bus, "host-owner", owner);

  acquireLease(bus, owner);
  publishUserMessage(owner, "pending");

  const late = new FakeHostChild();
  register(bus, "host-late", late);

  const invalidations = late
    .deliveries()
    .filter((event) => event.type === "task_snapshot_invalidated");
  assert.equal(invalidations.length, 1);
  assert.equal(invalidations[0]?.reason, "stream_mirror_gap");
  assert.deepEqual(invalidations[0]?.streamWatermark, { runId: RUN_ID, opSeq: 0 });
});

test("终态批次 flush 后删除，之后接入的 Host 不会拿到过期重放", async () => {
  const bus = new TaskRealtimeBus({ logger: silentLogger });
  const owner = new FakeHostChild();
  register(bus, "host-owner", owner);

  acquireLease(bus, owner);
  publishUserMessage(owner, "hello");
  publishTerminal(owner);
  // 终态 op 立即 flush，且批次在 terminal 后删除。
  await new Promise((resolve) => setTimeout(resolve, 0));

  const batches = owner
    .deliveries()
    .filter((event) => event.type === "task_stream_mirror_batch");
  assert.equal(batches.length, 1);
  assert.equal(batches[0]?.terminal, true);

  const late = new FakeHostChild();
  register(bus, "host-late", late);
  assert.equal(late.deliveries().length, 0);
});
