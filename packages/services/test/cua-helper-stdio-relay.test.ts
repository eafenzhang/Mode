import assert from "node:assert/strict";
import { Readable } from "node:stream";
import test from "node:test";

// helper 子进程 stdio 中继（spec: docs/specs/lan-remote-desktop-native-media.md 失败语义
// 「helper 侧 `[rd_media]` 运行告警必须接入主日志」）：Rust warn() 走 stderr eprintln，
// 此前 stdio 继承下打包态无消费者、日志零落点（2026-10-13 排查实证）。断言三个行为：
// [rd_media] 行进 warn、其余 stderr 进 debug、跨 chunk 行缓冲正确拼接。
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

test("[rd_media] stderr 行进 warn（运行期降级必须生产可见）", async () => {
  const { warns, debugs, logger } = captureLogger();
  const { child, stderr } = fakeChild();
  relayHelperChildStdio(child, logger, { generation: 3 });
  stderr.push("[rd_media] GPU 光标合成不可用，回退 CPU 回读路径: boom\n");
  stderr.push(null); // EOF → end 事件收口
  await flush();
  assert.equal(warns.length, 1, "rd_media 行恰好告警一次");
  assert.match(String(warns[0]?.[1]), /\[rd_media\]/u);
  assert.deepEqual(warns[0]?.[2], { source: "cua-helper-stderr", generation: 3, pid: 4242 });
  assert.equal(debugs.length, 0, "rd_media 行不重复进 debug");
});

test("其余 stderr 行进 debug（日志纪律：不进生产盘）", async () => {
  const { warns, debugs, logger } = captureLogger();
  const { child, stderr } = fakeChild();
  relayHelperChildStdio(child, logger, { generation: 1 });
  stderr.push("ordinary helper chatter\n");
  stderr.push(null);
  await flush();
  assert.equal(debugs.length, 1, "普通行恰好 debug 一次");
  assert.equal(warns.length, 0, "普通行不得升 warn");
});

test("跨 chunk 行缓冲拼接（半行不发、凑齐才发，\\r 归一）", async () => {
  const { warns, debugs, logger } = captureLogger();
  const { child, stderr } = fakeChild();
  relayHelperChildStdio(child, logger, { generation: 2 });
  stderr.push("[rd_media] 光标形状渲");
  stderr.push("染失败，本形状周期内跳过光标补丁\n[rd_media] 第二行\r\n");
  stderr.push(null);
  await flush();
  assert.equal(warns.length, 2, "两个 rd_media 行按 \\n 切分");
  assert.match(String(warns[0]?.[1]), /形状渲染失败/u);
  assert.match(String(warns[1]?.[1]), /第二行/u);
  assert.doesNotMatch(String(warns[1]?.[1]), /\r/u, "\\r 已归一");
  assert.equal(debugs.length, 0);
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
  // panic 不带 [rd_media] 前缀——归类错误会把它丢进 debug（生产不落盘），重蹈排查盲区。
  stderr.push(
    "thread 'rd-media-worker' panicked at crates/mode-cua-ax/src/rd_media/capture.rs:1:1:\n",
  );
  stderr.push(null);
  await flush();
  assert.equal(errors.length, 1, "panic 行恰好 error 一次");
  assert.match(String(errors[0]?.[1]), /panicked at/u);
  assert.equal(warns.length, 0);
  assert.equal(debugs.length, 0, "panic 不得降级进 debug");
});
