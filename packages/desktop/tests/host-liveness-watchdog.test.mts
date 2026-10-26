/**
 * Host 半开管道心跳的步进逻辑用例（spec `tauri-renderer-host.md` §22.5）。
 *
 * 调度器可注入：测试手动推进虚拟时钟，不依赖真实定时器。
 * 覆盖：健康路径不误杀、连续两次超时判定停摆且只回调一次、恢复重置 strike、stop 后不再探测。
 */
import assert from "node:assert/strict";
import test from "node:test";

import { startHostLivenessWatchdog } from "../src/renderer/src/hostLivenessWatchdog.js";

const flush = (): Promise<void> => new Promise((resolve) => setImmediate(resolve));

function createScheduler() {
  interface Task {
    at: number;
    cb: () => void;
    cancelled: boolean;
  }
  const tasks: Task[] = [];
  let now = 0;
  const schedule = (cb: () => void, ms: number): (() => void) => {
    const task: Task = { at: now + ms, cb, cancelled: false };
    tasks.push(task);
    return () => {
      task.cancelled = true;
    };
  };
  return {
    schedule,
    advance(ms: number): void {
      now += ms;
      for (const task of [...tasks].sort((a, b) => a.at - b.at)) {
        if (!task.cancelled && task.at <= now) {
          task.cancelled = true;
          task.cb();
        }
      }
    },
  };
}

interface Harness {
  scheduler: ReturnType<typeof createScheduler>;
  probes: number[];
  readonly stalled: number;
}

function startHarness(options?: { deadlineMs?: number; strikesToFail?: number }): Harness {
  const scheduler = createScheduler();
  const probes: number[] = [];
  let stalled = 0;
  startHostLivenessWatchdog({
    intervalMs: 30_000,
    deadlineMs: options?.deadlineMs ?? 10_000,
    strikesToFail: options?.strikesToFail ?? 2,
    ping: () => Promise.resolve(),
    onStalled: () => {
      stalled += 1;
    },
    onProbe: (ok, strikes) => probes.push(ok ? 0 : strikes),
    schedule: scheduler.schedule,
  });
  return {
    scheduler,
    probes,
    get stalled() {
      return stalled;
    },
  };
}

test("健康路径：ping 正常返回不误杀，且按间隔持续探测", async () => {
  const harness = startHarness();
  harness.scheduler.advance(30_000);
  await flush();
  harness.scheduler.advance(30_000);
  await flush();
  assert.deepEqual(harness.probes, [0, 0], "两次探测都应重置 strike");
  assert.equal(harness.stalled, 0);
});

test("连续两次超时 → 判定停摆且只回调一次，之后自停", async () => {
  let hang = true;
  const scheduler = createScheduler();
  const probes: number[] = [];
  let stalled = 0;
  startHostLivenessWatchdog({
    intervalMs: 30_000,
    deadlineMs: 10_000,
    strikesToFail: 2,
    ping: () => (hang ? new Promise<void>(() => {}) : Promise.resolve()),
    onStalled: () => {
      stalled += 1;
    },
    onProbe: (ok, strikes) => probes.push(ok ? 0 : strikes),
    schedule: scheduler.schedule,
  });

  scheduler.advance(30_000);
  await flush();
  scheduler.advance(10_000);
  await flush();
  scheduler.advance(30_000);
  await flush();
  scheduler.advance(10_000);
  await flush();

  assert.deepEqual(probes, [1, 2], "两次超时应各计一次 strike");
  assert.equal(stalled, 1, "达到阈值只回调一次 onStalled");

  // 自停：恢复 ping 也好、推进时钟也好，都不再有探测与回调。
  hang = false;
  scheduler.advance(60_000);
  await flush();
  assert.deepEqual(probes, [1, 2]);
  assert.equal(stalled, 1);
});

test("一次超时后恢复 → strike 重置，不升级为停摆", async () => {
  let hang = true;
  const scheduler = createScheduler();
  const probes: number[] = [];
  let stalled = 0;
  startHostLivenessWatchdog({
    intervalMs: 30_000,
    deadlineMs: 10_000,
    strikesToFail: 2,
    ping: () => (hang ? new Promise<void>(() => {}) : Promise.resolve()),
    onStalled: () => {
      stalled += 1;
    },
    onProbe: (ok, strikes) => probes.push(ok ? 0 : strikes),
    schedule: scheduler.schedule,
  });

  scheduler.advance(30_000);
  await flush();
  scheduler.advance(10_000);
  await flush();
  hang = false;
  scheduler.advance(30_000);
  await flush();

  assert.deepEqual(probes, [1, 0], "超时一次后恢复应重置 strike");
  assert.equal(stalled, 0);
});

test("stop() 后不再探测", async () => {
  const scheduler = createScheduler();
  const probes: number[] = [];
  const watchdog = startHostLivenessWatchdog({
    intervalMs: 30_000,
    deadlineMs: 10_000,
    strikesToFail: 2,
    ping: () => Promise.resolve(),
    onStalled: () => {},
    onProbe: (ok) => probes.push(ok ? 0 : 1),
    schedule: scheduler.schedule,
  });
  watchdog.stop();
  scheduler.advance(120_000);
  await flush();
  assert.deepEqual(probes, []);
});
