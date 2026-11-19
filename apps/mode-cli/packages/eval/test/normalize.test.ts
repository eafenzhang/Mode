// 规范化词表（spec §6）：id/time/路径/回环/UUID/shell 前缀/无序组/JSON 对象键序。
import assert from "node:assert/strict";
import test from "node:test";

import { normalizeValue, projectEventStream, stableStringify } from "../src/normalize.js";

const CONTEXT = {
  cwd: "/tmp/ws",
  dataRoot: "/tmp/data",
  tmpDir: "/tmp",
};

test("易变 id 键折叠成 <id>", () => {
  const value = normalizeValue(
    { sessionId: "sess_123", messageId: "m-1", providerId: "oracle-mock" },
    CONTEXT,
  );
  assert.deepEqual(value, { sessionId: "<id>", messageId: "<id>", providerId: "oracle-mock" });
});

test("时间键折叠成 <time>（含 ISO 字符串）", () => {
  const value = normalizeValue(
    { timestamp: "2026-01-01T00:00:00Z", durationMs: 12, name: "keep" },
    CONTEXT,
  );
  assert.deepEqual(value, { timestamp: "<time>", durationMs: "<time>", name: "keep" });
});

test("路径与回环 URL 折叠", () => {
  const value = normalizeValue(
    {
      path: "/tmp/ws/hello.txt",
      root: "/tmp/data/sub",
      url: "http://127.0.0.1:8123/v1/messages",
    },
    CONTEXT,
  );
  assert.deepEqual(value, {
    path: "<cwd>/hello.txt",
    root: "<data-root>/sub",
    url: "<loopback-url>/v1/messages",
  });
});

test("UUID 与 shell 前缀折叠", () => {
  const value = normalizeValue(
    {
      text: "anchor 0f4c2b1a-1234-4c5d-8e9f-001122334455 ok",
      error: "bash: line 3: nope: command not found",
    },
    CONTEXT,
  );
  assert.deepEqual(value, {
    text: "anchor <id> ok",
    error: "<shell>: line <n>: nope: command not found",
  });
});

test("stack 删除、id 列表折叠、id 容器保结构", () => {
  const value = normalizeValue(
    {
      stack: "Error: x\n    at f (file.ts:1:1)",
      toolCallIds: ["a", "b"],
      parallelGroups: [["a"], ["b", "c"]],
    },
    CONTEXT,
  );
  assert.deepEqual(value, { toolCallIds: "<ids>", parallelGroups: [["<id>"], ["<id>", "<id>"]] });
});

test("JSON 文本里的对象键序稳定", () => {
  const value = normalizeValue({ delta: '{"b":1,"a":2}' }, CONTEXT);
  assert.deepEqual(value, { delta: '{"a":2,"b":1}' });
});

test("连续 session.updated 无序组折叠为排序后集合", () => {
  const events = [
    { type: "turn.started", payload: { turnId: "t" } },
    { type: "session.updated", payload: { revision: 3 } },
    { type: "session.updated", payload: { revision: 1 } },
    { type: "turn.completed", payload: { turnId: "t" } },
  ];
  const projected = projectEventStream(events, CONTEXT);
  assert.deepEqual(
    projected.map((event) => (event.payload as { revision?: number }).revision),
    [undefined, 1, 3, undefined],
  );
});

test("单次工具完成不折叠（顺序是行为）", () => {
  const events = [
    { type: "tool.updated", payload: { kind: "result", toolCallId: "a", result: { success: true } } },
    { type: "tool.updated", payload: { kind: "error", toolCallId: "b", error: { message: "x" } } },
  ];
  const projected = projectEventStream(events, CONTEXT);
  // 两个不同调用的完成段是并行批次（≥2 个不同 toolCallId），折叠后按稳定串排序。
  const kinds = projected.map((event) => (event.payload as { kind: string }).kind);
  assert.deepEqual([...kinds].sort(), ["error", "result"]);
});

test("stableStringify 键排序", () => {
  assert.equal(stableStringify({ b: 1, a: { d: 2, c: 3 } }), '{"a":{"c":3,"d":2},"b":1}');
});
