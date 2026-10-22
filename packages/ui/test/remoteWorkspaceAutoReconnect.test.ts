import assert from "node:assert/strict";
import test from "node:test";

import {
  classifyRemoteSessionClose,
  createRemoteAutoReconnectScheduler,
  REMOTE_AUTO_RECONNECT_DELAYS_MS,
  type RemoteAutoReconnectAttemptResult,
} from "../src/lib/remoteWorkspaceAutoReconnect.js";

// 产品决策 B：断连后有限自动重连（1s/3s/9s，最多 3 次）；干净退出与鉴权/版本类错误不重试。

test("断连分类：transport/crash 可重试，clean-exit 与鉴权/版本错误不重试", () => {
  assert.deepEqual(classifyRemoteSessionClose({ exitCode: null, signal: null }), {
    retryable: true,
    detail: "transport",
  });
  assert.deepEqual(classifyRemoteSessionClose({ exitCode: 1, signal: null }), {
    retryable: true,
    detail: "crash",
  });
  assert.deepEqual(classifyRemoteSessionClose({ exitCode: null, signal: "SIGKILL" }), {
    retryable: true,
    detail: "crash",
  });
  assert.deepEqual(classifyRemoteSessionClose({ exitCode: 0, signal: null }), {
    retryable: false,
    detail: "clean-exit",
  });
  assert.deepEqual(
    classifyRemoteSessionClose({ exitCode: null, signal: null, error: "unauthorized: bad token" }),
    { retryable: false, detail: "non-retryable-error" },
  );
  assert.deepEqual(
    classifyRemoteSessionClose({
      exitCode: null,
      signal: null,
      error: "protocol version mismatch (host 3, client 1)",
    }),
    { retryable: false, detail: "non-retryable-error" },
  );
  assert.deepEqual(
    classifyRemoteSessionClose({ exitCode: null, signal: null, error: "connection reset by peer" }),
    { retryable: true, detail: "unknown-error" },
  );
});

test("退避表固定为 1s/3s/9s，共 3 次尝试", () => {
  assert.deepEqual([...REMOTE_AUTO_RECONNECT_DELAYS_MS], [1_000, 3_000, 9_000]);
});

function fakeTimers() {
  let nextId = 1;
  const pending = new Map<number, { run: () => void; ms: number }>();
  return {
    setTimeout: (handler: () => void, ms: number) => {
      const id = nextId;
      nextId += 1;
      pending.set(id, { run: handler, ms });
      return id;
    },
    clearTimeout: (handle: unknown) => {
      pending.delete(Number(handle));
    },
    pendingDelays: () => [...pending.values()].map((entry) => entry.ms),
    async runNext(): Promise<void> {
      const next = pending.entries().next();
      assert.notEqual(next.done, true, "没有待执行的定时器");
      const [id, entry] = next.value as [number, { run: () => void; ms: number }];
      pending.delete(id);
      entry.run();
      // `fire` 是 async：多放行两轮微任务，让 attempt 与后续调度落地。
      await new Promise((resolve) => setImmediate(resolve));
      await new Promise((resolve) => setImmediate(resolve));
    },
  };
}

test("三次失败后触底：按 1s/3s/9s 依次尝试并带出最后一次错误", async () => {
  const timers = fakeTimers();
  const attempts: number[] = [];
  const failures: Array<[number, string]> = [];
  let exhausted: string | null = null;
  const scheduler = createRemoteAutoReconnectScheduler({
    setTimeout: timers.setTimeout,
    clearTimeout: timers.clearTimeout,
    attempt: async (_key, attemptIndex): Promise<RemoteAutoReconnectAttemptResult> => {
      attempts.push(attemptIndex);
      return { ok: false, error: `attempt-${attemptIndex}-failed` };
    },
    onAttemptFailed: (_key, index, error) => failures.push([index, error]),
    onExhausted: (_key, error) => {
      exhausted = error;
    },
  });

  scheduler.schedule("ws-key");
  assert.equal(scheduler.isScheduled("ws-key"), true);
  assert.deepEqual(timers.pendingDelays(), [1_000]);

  await timers.runNext();
  assert.deepEqual(attempts, [0]);
  assert.deepEqual(timers.pendingDelays(), [3_000]);

  await timers.runNext();
  assert.deepEqual(attempts, [0, 1]);
  assert.deepEqual(timers.pendingDelays(), [9_000]);

  await timers.runNext();
  assert.deepEqual(attempts, [0, 1, 2]);
  assert.equal(scheduler.isScheduled("ws-key"), false);
  assert.deepEqual(timers.pendingDelays(), []);
  assert.deepEqual(failures, [
    [0, "attempt-0-failed"],
    [1, "attempt-1-failed"],
    [2, "attempt-2-failed"],
  ]);
  assert.equal(exhausted, "attempt-2-failed");
});

test("中途成功即停止；不触发 onExhausted", async () => {
  const timers = fakeTimers();
  const attempts: number[] = [];
  let exhausted = 0;
  const scheduler = createRemoteAutoReconnectScheduler({
    setTimeout: timers.setTimeout,
    clearTimeout: timers.clearTimeout,
    attempt: async (_key, attemptIndex) => {
      attempts.push(attemptIndex);
      return { ok: attemptIndex === 1 };
    },
    onExhausted: () => {
      exhausted += 1;
    },
  });

  scheduler.schedule("ws-key");
  await timers.runNext();
  assert.deepEqual(attempts, [0]);
  await timers.runNext();
  assert.deepEqual(attempts, [0, 1]);
  assert.equal(scheduler.isScheduled("ws-key"), false);
  assert.deepEqual(timers.pendingDelays(), []);
  assert.equal(exhausted, 0);
});

test("cancel 阻止未执行的尝试；执行中 cancel 不再排下一次", async () => {
  const timers = fakeTimers();
  const attempts: number[] = [];
  const scheduler = createRemoteAutoReconnectScheduler({
    setTimeout: timers.setTimeout,
    clearTimeout: timers.clearTimeout,
    attempt: async (_key, attemptIndex) => {
      attempts.push(attemptIndex);
      // 第一次尝试执行期间被取消（例如 Bot 已重连成功）。
      scheduler.cancel("ws-key");
      return { ok: false, error: "late failure" };
    },
  });

  scheduler.schedule("ws-key");
  scheduler.cancel("ws-key");
  assert.deepEqual(timers.pendingDelays(), []);
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(attempts, [], "cancel 后不得再执行已取消的尝试");

  scheduler.schedule("ws-key");
  await timers.runNext();
  assert.deepEqual(attempts, [0]);
  assert.equal(scheduler.isScheduled("ws-key"), false, "执行中取消后不得再排下一次");
  assert.deepEqual(timers.pendingDelays(), []);
});

test("重复 schedule 替换旧链，只保留一次尝试", async () => {
  const timers = fakeTimers();
  const attempts: number[] = [];
  const scheduler = createRemoteAutoReconnectScheduler({
    setTimeout: timers.setTimeout,
    clearTimeout: timers.clearTimeout,
    attempt: async (_key, attemptIndex) => {
      attempts.push(attemptIndex);
      return { ok: true };
    },
  });

  scheduler.schedule("ws-key");
  scheduler.schedule("ws-key");
  assert.deepEqual(timers.pendingDelays(), [1_000], "重复调度不得留下两条链");
  await timers.runNext();
  assert.deepEqual(attempts, [0]);
  assert.deepEqual(timers.pendingDelays(), []);
});
