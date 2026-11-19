// diff（spec §3）：两次 run 的确定性门禁与差异定位（turn/tool）。
import assert from "node:assert/strict";
import test from "node:test";
import { readFile, rm, mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { diffRuns } from "../src/diff.js";
import { runSuite } from "../src/runner.js";
import { createRedactor } from "../src/redact.js";
import type { EvalCase, EvalEvent, RunReport, SessionFactory } from "../src/types.js";

const BASE_CASE: EvalCase = {
  case: "read-single",
  prompt: "读一下 hello.txt",
  seed: { "hello.txt": "hello\n" },
  script: [
    { toolCalls: [{ name: "Read", input: { file_path: "hello.txt" } }] },
    { text: "done" },
  ],
};

/** 两个 turn 的确定性事件流：turn 0 一次 Read，turn 1 收尾（无工具）。 */
function fakeEvents(): EvalEvent[] {
  return [
    { type: "turn.started", payload: { turnId: "t-1", messageId: "m-1" } },
    { type: "model.streaming", payload: { kind: "tool_call", toolCallId: "call-1", input: { file_path: "hello.txt" } } },
    { type: "tool.updated", payload: { kind: "scheduled", toolName: "Read", toolCallId: "call-1" } },
    { type: "tool.updated", payload: { kind: "result", toolCallId: "call-1", result: { success: true, content: "hello\n" } } },
    { type: "turn.completed", payload: { turnId: "t-1" } },
    { type: "turn.started", payload: { turnId: "t-2", messageId: "m-2" } },
    { type: "message.updated", payload: { role: "assistant", text: "done" } },
    { type: "turn.completed", payload: { turnId: "t-2" } },
  ];
}

const redactor = createRedactor();

function fakeSessionFactory(): SessionFactory {
  return {
    async start() {
      const events = fakeEvents();
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
}

async function runOnce(dir: string): Promise<RunReport> {
  return await runSuite({
    suite: { schemaVersion: 1, name: "demo", cases: [{ ...BASE_CASE }] },
    suitePath: "/tmp/demo.json",
    outDir: dir,
    sessionFactory: fakeSessionFactory(),
    redactor,
  });
}

test("轨迹行携带 turn：第二个 turn 的事件 turn=1", async () => {
  const dir = await mkdtemp(join(tmpdir(), "mode-eval-turn-"));
  try {
    const report = await runOnce(dir);
    const text = await readFile(report.results[0]?.trajectoryPath as string, "utf8");
    const lines = text.split("\n").filter((line) => line.includes('"kind":"event"'));
    const turns = lines.map((line) => (JSON.parse(line) as { turn?: number }).turn);
    assert.deepEqual(turns, [0, 0, 0, 0, 0, 1, 1, 1]);
    const toolLines = text.split("\n").filter((line) => line.includes('"kind":"toolCall"'));
    assert.equal((JSON.parse(toolLines[0] as string) as { turn?: number }).turn, 0);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("两次 run diff → identical", async () => {
  const root = await mkdtemp(join(tmpdir(), "mode-eval-diff-"));
  try {
    await runOnce(join(root, "run1"));
    await runOnce(join(root, "run2"));
    const diff = await diffRuns(join(root, "run1"), join(root, "run2"));
    assert.equal(diff.identical, true);
    assert.equal(diff.cases.length, 1);
    assert.deepEqual(diff.cases[0]?.mismatches, []);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("篡改 run2 的事件 → 差异定位到 turn", async () => {
  const root = await mkdtemp(join(tmpdir(), "mode-eval-diff-tamper-"));
  try {
    await runOnce(join(root, "run1"));
    await runOnce(join(root, "run2"));
    const path = join(root, "run2", "trajectories", "read-single.jsonl");
    await rewriteLine(path, (line) => {
      if (line.kind === "event" && line.type === "message.updated") {
        return { ...line, payload: { role: "assistant", text: "DONE" } };
      }
      return line;
    });
    const diff = await diffRuns(join(root, "run1"), join(root, "run2"));
    assert.equal(diff.identical, false);
    const mismatch = diff.cases[0]?.mismatches[0];
    assert.ok(mismatch);
    assert.match(mismatch.path, /^events\[\d+\]/);
    assert.equal(mismatch.location, "turn=1");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("篡改 run2 的工具输出 → 差异定位到 tool", async () => {
  const root = await mkdtemp(join(tmpdir(), "mode-eval-diff-tool-"));
  try {
    await runOnce(join(root, "run1"));
    await runOnce(join(root, "run2"));
    const path = join(root, "run2", "trajectories", "read-single.jsonl");
    await rewriteLine(path, (line) => {
      if (line.kind === "toolCall") {
        return { ...line, output: "goodbye\n" };
      }
      return line;
    });
    const diff = await diffRuns(join(root, "run1"), join(root, "run2"));
    assert.equal(diff.identical, false);
    const mismatch = diff.cases[0]?.mismatches[0];
    assert.ok(mismatch);
    assert.match(mismatch.path, /^toolCalls\[\d+\]/);
    assert.equal(mismatch.location, "tool=Read#0");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

/** 轨迹是 JSONL；按行解析后重写第一处命中的行（保持行序与格式）。 */
async function rewriteLine(
  path: string,
  rewrite: (line: Record<string, unknown>) => Record<string, unknown>,
): Promise<void> {
  const text = await readFile(path, "utf8");
  const lines = text.split("\n").filter((line) => line.trim().length > 0);
  let hit = false;
  const out = lines.map((line) => {
    if (hit) return line;
    const parsed = JSON.parse(line) as Record<string, unknown>;
    const rewritten = rewrite(parsed);
    if (rewritten !== parsed) {
      hit = true;
      return JSON.stringify(rewritten);
    }
    return line;
  });
  assert.ok(hit, "no line matched the rewrite");
  await writeFile(path, `${out.join("\n")}\n`, "utf8");
}

test("只在一侧存在的 case → onlyIn", async () => {
  const root = await mkdtemp(join(tmpdir(), "mode-eval-diff-missing-"));
  try {
    await runOnce(join(root, "run1"));
    await runOnce(join(root, "run2"));
    await rm(join(root, "run2", "trajectories", "read-single.jsonl"), { force: true });
    const diff = await diffRuns(join(root, "run1"), join(root, "run2"));
    assert.equal(diff.identical, false);
    assert.equal(diff.cases[0]?.onlyIn, "A");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
