// Runner 端到端（fake session factory）：确定性、比较、录制、轨迹落盘与回放。
// 这批测试不 spawn 子进程；真实 agent-server 的协议路径由 opt-in 集成测试与手动 smoke 覆盖。
import assert from "node:assert/strict";
import test from "node:test";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { createRedactor } from "../src/redact.js";
import { replayTrajectory, runSuite } from "../src/runner.js";
import { readTrajectory, toOracleFixture } from "../src/trajectory.js";
import type { EvalCase, EvalEvent, SessionFactory } from "../src/types.js";

const CASE_ID = "read-single";

const BASE_CASE: EvalCase = {
  case: CASE_ID,
  prompt: "读一下 hello.txt",
  seed: { "hello.txt": "hello\n" },
  script: [
    { toolCalls: [{ name: "Read", input: { file_path: "hello.txt" } }] },
    { text: "done" },
  ],
};

/** 与 agent-server 事件面同形的确定性假事件流（含易变 id 与乱序状态推送）。 */
function fakeEvents(seedRevision: number): EvalEvent[] {
  return [
    { type: "turn.started", payload: { turnId: `t-${seedRevision}`, messageId: `m-${seedRevision}` } },
    { type: "session.updated", payload: { sessionId: "sess_1", revision: 2 } },
    { type: "session.updated", payload: { sessionId: "sess_1", revision: 1 } },
    { type: "model.streaming", payload: { kind: "tool_call", toolCallId: "call-1", input: { file_path: "hello.txt" } } },
    { type: "tool.updated", payload: { kind: "scheduled", toolName: "Read", toolCallId: "call-1" } },
    { type: "tool.updated", payload: { kind: "started", toolCallId: "call-1" } },
    { type: "tool.updated", payload: { kind: "result", toolCallId: "call-1", result: { success: true, content: "hello\n" } } },
    { type: "message.updated", payload: { role: "assistant", text: "done" } },
    { type: "turn.completed", payload: { turnId: "t-1" } },
  ];
}

function fakeSessionFactory(): SessionFactory & { runs: number } {
  const impl = {
    runs: 0,
    async start() {
      impl.runs += 1;
      const revision = impl.runs;
      const events = fakeEvents(revision);
      return {
        async send(_prompt: string) {},
        events,
        get stderr() {
          return "";
        },
        invalidLines: [],
        async stop() {},
      };
    },
  };
  return impl;
}

const redactor = createRedactor();

const HELLO_SHA = createHash("sha256").update("hello\n").digest("hex");

const EXPECTED = {
  toolCalls: [{ name: "Read", input: { file_path: "hello.txt" }, ok: true, output: "hello\n" }],
  files: { "hello.txt": HELLO_SHA },
  events: [
    { type: "turn.started", payload: { turnId: "<id>", messageId: "<id>" } },
    // 连续 session.updated 无序组：折叠后按稳定串排序，revision 1 在前。
    { type: "session.updated", payload: { sessionId: "<id>", revision: 1 } },
    { type: "session.updated", payload: { sessionId: "<id>", revision: 2 } },
    {
      type: "model.streaming",
      payload: { kind: "tool_call", toolCallId: "<id>", input: { file_path: "hello.txt" } },
    },
    { type: "tool.updated", payload: { kind: "scheduled", toolName: "Read", toolCallId: "<id>" } },
    { type: "tool.updated", payload: { kind: "started", toolCallId: "<id>" } },
    {
      type: "tool.updated",
      payload: { kind: "result", toolCallId: "<id>", result: { success: true, content: "hello\n" } },
    },
    { type: "message.updated", payload: { role: "assistant", text: "done" } },
    { type: "turn.completed", payload: { turnId: "<id>" } },
  ],
};

async function withTempDir(fn: (dir: string) => Promise<void>): Promise<void> {
  const dir = await mkdtemp(join(tmpdir(), "mode-eval-runner-"));
  try {
    await fn(dir);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}

test("expect 命中 → passed；轨迹落盘且去掉 timing 后稳定", async () => {
  await withTempDir(async (dir) => {
    const factory = fakeSessionFactory();
    const suite = { schemaVersion: 1 as const, name: "demo", cases: [{ ...BASE_CASE, expect: EXPECTED }] };
    const report1 = await runSuite({
      suite,
      suitePath: "/tmp/demo.json",
      outDir: dir,
      sessionFactory: factory,
      redactor,
    });
    assert.equal(report1.totals.passed, 1);
    assert.equal(report1.totals.failed, 0);

    const trajectoryPath = report1.results[0]?.trajectoryPath as string;
    const text1 = await readFile(trajectoryPath, "utf8");

    // 第二次 run：两次结果（去 timing）逐字节一致。
    const report2 = await runSuite({
      suite,
      suitePath: "/tmp/demo.json",
      outDir: join(dir, "again"),
      sessionFactory: fakeSessionFactory(),
      redactor,
    });
    assert.deepEqual(
      JSON.stringify(report1.results[0]?.mismatches),
      JSON.stringify(report2.results[0]?.mismatches),
    );
    const text2 = await readFile(report2.results[0]?.trajectoryPath as string, "utf8");
    assert.equal(text1, text2);
  });
});

test("篡改 expect → failed 且给出差异点", async () => {
  await withTempDir(async (dir) => {
    const tampered = {
      ...EXPECTED,
      toolCalls: [{ ...EXPECTED.toolCalls[0], output: "helloX\n" }],
    };
    const suite = { schemaVersion: 1 as const, name: "demo", cases: [{ ...BASE_CASE, expect: tampered }] };
    const report = await runSuite({
      suite,
      suitePath: "/tmp/demo.json",
      outDir: dir,
      sessionFactory: fakeSessionFactory(),
      redactor,
    });
    assert.equal(report.totals.failed, 1);
    const mismatches = report.results[0]?.mismatches ?? [];
    assert.ok(mismatches.length > 0);
    assert.ok(mismatches.some((m) => m.path.includes("output")));
  });
});

test("无 expect → recorded，轨迹仍完整", async () => {
  await withTempDir(async (dir) => {
    const suite = { schemaVersion: 1 as const, name: "demo", cases: [{ ...BASE_CASE }] };
    const report = await runSuite({
      suite,
      suitePath: "/tmp/demo.json",
      outDir: dir,
      sessionFactory: fakeSessionFactory(),
      redactor,
    });
    assert.equal(report.totals.recorded, 1);
    const trajectory = await readTrajectory(report.results[0]?.trajectoryPath as string);
    assert.equal(trajectory.result?.status, "recorded");
    assert.equal(trajectory.toolCalls.length, 1);
    assert.equal(Object.keys(trajectory.files).length, 1);
  });
});

test("轨迹 round-trip 与 toOracleFixture 形状", async () => {
  await withTempDir(async (dir) => {
    const suite = { schemaVersion: 1 as const, name: "demo", cases: [{ ...BASE_CASE, expect: EXPECTED }] };
    const report = await runSuite({
      suite,
      suitePath: "/tmp/demo.json",
      outDir: dir,
      sessionFactory: fakeSessionFactory(),
      redactor,
    });
    const trajectory = await readTrajectory(report.results[0]?.trajectoryPath as string);
    const fixture = toOracleFixture(trajectory);
    assert.equal(fixture.case, CASE_ID);
    assert.deepEqual(fixture.seed, { "hello.txt": "hello\n" });
    assert.deepEqual(fixture.expect.files, { "hello.txt": HELLO_SHA });
    assert.equal(fixture.expect.toolCalls[0]?.name, "Read");
    assert.equal(fixture.expect.events.length, EXPECTED.events.length);
  });
});

test("replayTrajectory 复跑同一输入 → passed", async () => {
  await withTempDir(async (dir) => {
    const suite = { schemaVersion: 1 as const, name: "demo", cases: [{ ...BASE_CASE, expect: EXPECTED }] };
    const first = await runSuite({
      suite,
      suitePath: "/tmp/demo.json",
      outDir: dir,
      sessionFactory: fakeSessionFactory(),
      redactor,
    });
    const result = await replayTrajectory(first.results[0]?.trajectoryPath as string, {
      sessionFactory: fakeSessionFactory(),
      redactor,
    });
    assert.equal(result.status, "passed");
  });
});

test("脱敏先于落盘：事件里的 secret 不出现在轨迹", async () => {
  await withTempDir(async (dir) => {
    const leakyFactory: SessionFactory = {
      async start() {
        const events = [
          ...fakeEvents(1),
          {
            type: "message.updated",
            payload: { role: "assistant", text: "leak sk-abcdef1234567890 now" },
          },
          { type: "turn.completed", payload: { turnId: "t" } },
        ];
        return {
          async send(_prompt: string) {},
          events,
          get stderr() {
            return "";
          },
          invalidLines: [],
          async stop() {},
        };
      },
    };
    const suite = { schemaVersion: 1 as const, name: "demo", cases: [{ ...BASE_CASE, expect: EXPECTED }] };
    const report = await runSuite({
      suite,
      suitePath: "/tmp/demo.json",
      outDir: dir,
      sessionFactory: leakyFactory,
      redactor,
    });
    const text = await readFile(report.results[0]?.trajectoryPath as string, "utf8");
    assert.ok(!text.includes("sk-abcdef1234567890"));
    assert.ok(text.includes("<redacted-key>"));
  });
});
