import assert from "node:assert/strict";
import { Readable } from "node:stream";
import test from "node:test";

// helper 子进程 stdio 中继：Rust panic 走 stderr eprintln，若 stdio 继承则打包态无消费者、
// 日志零落点（2026-10-13 排查实证）。断言三个行为：panic 行升 error、其余 stderr 行进 debug、
// 跨 chunk 行缓冲正确拼接。
const { relayHelperChildStdio } = await import(
  "../src/cua-permission-broker/windowsCuaHelperHostSupport.js"
);
const { createServiceLogger } = await import("../src/logger/serviceLogger.js");

type HelperChild = Parameters<typeof relayHelperChildStdio>[0];

interface Capture {
  warns: unknown[][];
  debugs: unknown[][];
  errors: unknown[][];
  logger: ReturnType<typeof createServiceLogger>;
}

function captureLogger(): Capture {
  const warns: unknown[][] = [];
  const debugs: unknown[][] = [];
  const errors: unknown[][] = [];
  const logger = createServiceLogger("test-relay", {
    isDebugEnabled: true,
    sink: {
      log: () => undefined,
      warn: (...args: unknown[]) => warns.push(args),
      error: (...args: unknown[]) => errors.push(args),
      debug: (...args: unknown[]) => debugs.push(args),
    },
  });
  return { warns, debugs, errors, logger };
}

function fakeChild() {
  // Readable 充当 fork(silent) 的 stderr/stdout 管道（push 模拟子进程写入）。
  const stderr = new Readable({ read: () => undefined });
  const stdout = new Readable({ read: () => undefined });
  const child = { pid: 4242, stderr, stdout, send: () => true, kill: () => true };
  return { child: child as unknown as HelperChild, stderr, stdout };
}

/** 等流事件跑完（push → data/end 的同步/异步实现差异由一个宏任务抹平）。 */
const flush = (): Promise<void> => new Promise((resolve) => setImmediate(resolve));

test("其余 stderr 行进 debug（日志纪律：不进生产盘）", async () => {
  const { warns, debugs, logger } = captureLogger();
  const { child, stderr } = fakeChild();
  relayHelperChildStdio(child, logger, { generation: 1 });
  stderr.push("ordinary helper chatter\n");
  stderr.push(null);
  await flush();
  assert.equal(debugs.length, 1, "普通行恰好 debug 一次");
  assert.deepEqual(debugs[0]?.[2], { source: "cua-helper-stderr", generation: 1, pid: 4242 });
  assert.equal(warns.length, 0, "普通行不得升 warn");
});

test("跨 chunk 行缓冲拼接（半行不发、凑齐才发，\\r 归一）", async () => {
  const { warns, debugs, logger } = captureLogger();
  const { child, stderr } = fakeChild();
  relayHelperChildStdio(child, logger, { generation: 2 });
  stderr.push("第一行前");
  stderr.push("半段，凑齐才发\n第二行\r\n");
  stderr.push(null);
  await flush();
  assert.equal(debugs.length, 2, "两行按 \\n 切分");
  assert.match(String(debugs[0]?.[1]), /第一行前半段，凑齐才发/u);
  assert.match(String(debugs[1]?.[1]), /第二行/u);
  assert.doesNotMatch(String(debugs[1]?.[1]), /\r/u, "\\r 已归一");
  assert.equal(warns.length, 0);
});

test("stdout 排空；无流（stdio 继承假件）空操作不抛", () => {
  const { logger } = captureLogger();
  const { child, stdout } = fakeChild();
  relayHelperChildStdio(child, logger, { generation: 1 });
  // resume 后写入不得积压（排空语义由 Readable 内部保证，这里确认不抛即可）。
  stdout.push("ignored stdout\n");
  stdout.push(null);
  // 无 stderr/stdout（stdio 继承的测试假件）→ 空操作。
  const bare = { pid: 7, send: () => true, kill: () => true } as unknown as HelperChild;
  relayHelperChildStdio(bare, logger, { generation: 1 });
});

test("Rust panic 行升 error 落盘（worker 死因不得零痕迹）", async () => {
  const { warns, debugs, errors, logger } = captureLogger();
  const { child, stderr } = fakeChild();
  relayHelperChildStdio(child, logger, { generation: 2 });
  // panic 行按子串归类为 error——归类错误会把它丢进 debug（生产不落盘），重蹈排查盲区。
  stderr.push("thread 'mode-cua-helper' panicked at crates/mode-cua-ax/src/capture.rs:1:1:\n");
  stderr.push(null);
  await flush();
  assert.equal(errors.length, 1, "panic 行恰好 error 一次");
  assert.match(String(errors[0]?.[1]), /panicked at/u);
  assert.equal(warns.length, 0);
  assert.equal(debugs.length, 0, "panic 不得降级进 debug");
});
