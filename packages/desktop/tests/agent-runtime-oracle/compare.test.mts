import { test } from "node:test";
import assert from "node:assert/strict";

import {
  compareEventStream,
  compareFileSnapshot,
  compareToolCalls,
  formatDiffs,
} from "./compare.mjs";
import type { OracleEvent } from "./driver.mjs";
import { projectEventStream, type NormalizeContext, type ToolCall } from "./normalize.mjs";

// 比较器是这一阶段的验收仪器：一个恒真的比较器会让「行为对拍通过」变成一句空话。
// 这一组用例只针对第三层投影与比较器本身（纯函数，不跑 oracle），钉住两件事：
// 差异必须被报出来；被豁免的顺序**只**是 spec §3 列出的那些。

const readCall: ToolCall = {
  name: "Read",
  input: { file_path: "<cwd>/hello.txt" },
  ok: true,
  output: "hello",
};

const CONTEXT: NormalizeContext = {
  dataRoot: "C:\\tmp\\mode-oracle-fixture-X",
  cwd: "C:\\tmp\\mode-oracle-fixture-ws-Y",
  tmpDir: "C:\\tmp",
};

function resultEvent(toolCallId: string, content: string): OracleEvent {
  return {
    type: "tool.updated",
    payload: {
      kind: "result",
      toolCallId,
      toolName: "Read",
      result: { success: true, content },
    },
  };
}

function batchEvent(toolCallId: string): OracleEvent {
  return {
    type: "tool.updated",
    payload: { kind: "batch", toolCallIds: [toolCallId], successCount: 1, errorCount: 0 },
  };
}

function scheduledEvent(toolCallId: string, filePath: string): OracleEvent {
  return {
    type: "tool.updated",
    payload: { kind: "scheduled", toolCallId, toolName: "Read", input: { file_path: filePath } },
  };
}

test("第一层：完全相同的工具调用序列没有差异", () => {
  assert.deepEqual(compareToolCalls([readCall], [{ ...readCall }]), []);
});

test("第一层：工具名 / 入参 / 成败 / 输出 / 条数的差异都被报出来", () => {
  assert.match(
    formatDiffs(compareToolCalls([{ ...readCall, name: "Write" }], [readCall])),
    /工具名不同/,
  );
  assert.match(
    formatDiffs(
      compareToolCalls([{ ...readCall, input: { file_path: "<cwd>/other.txt" } }], [readCall]),
    ),
    /工具入参不同/,
  );
  assert.match(
    formatDiffs(compareToolCalls([{ ...readCall, ok: false }], [readCall])),
    /工具成败不同/,
  );
  assert.match(
    formatDiffs(compareToolCalls([{ ...readCall, output: "" }], [readCall])),
    /工具输出不同/,
  );
  assert.match(formatDiffs(compareToolCalls([], [readCall])), /工具调用条数不同/);
});

test("第二层：多出 / 缺少 / 内容变化都被报出来", () => {
  assert.deepEqual(compareFileSnapshot({ "a.txt": "aa" }, { "a.txt": "aa" }), []);
  assert.match(
    formatDiffs(compareFileSnapshot({ "a.txt": "aa", "b.txt": "bb" }, { "a.txt": "aa" })),
    /多出文件/,
  );
  assert.match(
    formatDiffs(compareFileSnapshot({ "a.txt": "aa" }, { "a.txt": "aa", "b.txt": "bb" })),
    /缺少文件/,
  );
  assert.match(
    formatDiffs(compareFileSnapshot({ "a.txt": "zz" }, { "a.txt": "aa" })),
    /内容哈希不同/,
  );
});

test("第三层：事件类型 / payload / 条数的差异都被报出来", () => {
  assert.deepEqual(
    compareEventStream(
      [{ type: "turn.started", payload: {} }],
      [{ type: "turn.started", payload: {} }],
    ),
    [],
  );
  assert.match(
    formatDiffs(
      compareEventStream(
        [{ type: "turn.failed", payload: {} }],
        [{ type: "turn.started", payload: {} }],
      ),
    ),
    /事件类型不同/,
  );
  assert.match(
    formatDiffs(
      compareEventStream(
        [{ type: "turn.started", payload: { turnNumber: 1 } }],
        [{ type: "turn.started", payload: { turnNumber: 0 } }],
      ),
    ),
    /payload 不同/,
  );
  assert.match(
    formatDiffs(
      compareEventStream(
        [
          { type: "turn.started", payload: {} },
          { type: "turn.completed", payload: {} },
        ],
        [{ type: "turn.started", payload: {} }],
      ),
    ),
    /事件条数不同/,
  );
});

test("无序组：连续同类型的 session.updated 顺序差异被折叠掉", () => {
  const left = projectEventStream(
    [
      { type: "session.updated", payload: { a: 1 } },
      { type: "session.updated", payload: { b: 2 } },
      { type: "turn.started", payload: {} },
    ],
    CONTEXT,
  );
  const right = projectEventStream(
    [
      { type: "session.updated", payload: { b: 2 } },
      { type: "session.updated", payload: { a: 1 } },
      { type: "turn.started", payload: {} },
    ],
    CONTEXT,
  );
  assert.deepEqual(compareEventStream(left, right), []);
});

test("并行批次：同一批里两个工具调用的完成顺序被折叠（spec §3）", () => {
  const left = projectEventStream(
    [
      resultEvent("call_a", "1\talpha\n2\t"),
      batchEvent("call_a"),
      resultEvent("call_b", "1\tbeta\n2\t"),
      batchEvent("call_b"),
    ],
    CONTEXT,
  );
  const right = projectEventStream(
    [
      resultEvent("call_b", "1\tbeta\n2\t"),
      batchEvent("call_b"),
      resultEvent("call_a", "1\talpha\n2\t"),
      batchEvent("call_a"),
    ],
    CONTEXT,
  );
  assert.deepEqual(
    compareEventStream(left, right),
    [],
    "同一 assistant 消息里并行批次的完成顺序不是行为，应当被折叠",
  );
});

test("并行批次：折叠只发生在完成类事件之间，工具内容差异仍然报错", () => {
  const left = projectEventStream(
    [resultEvent("call_a", "alpha"), batchEvent("call_a"), resultEvent("call_b", "beta")],
    CONTEXT,
  );
  const right = projectEventStream(
    [resultEvent("call_a", "alpha"), batchEvent("call_a"), resultEvent("call_b", "BETA")],
    CONTEXT,
  );
  assert.match(
    formatDiffs(compareEventStream(left, right)),
    /payload 不同/,
    "折叠不可以把工具输出的差异一起吃掉",
  );
});

test("并行批次：单次调用的 result/batch 先后、以及调度顺序都不折叠", () => {
  const singleCallLeft = projectEventStream(
    [resultEvent("call_a", "x"), batchEvent("call_a")],
    CONTEXT,
  );
  const singleCallRight = projectEventStream(
    [batchEvent("call_a"), resultEvent("call_a", "x")],
    CONTEXT,
  );
  assert.notDeepEqual(
    compareEventStream(singleCallLeft, singleCallRight),
    [],
    "只有一个工具调用时 result/batch 的先后是行为，必须被比较",
  );

  const scheduleLeft = projectEventStream(
    [scheduledEvent("call_a", "<cwd>/a.txt"), scheduledEvent("call_b", "<cwd>/b.txt")],
    CONTEXT,
  );
  const scheduleRight = projectEventStream(
    [scheduledEvent("call_b", "<cwd>/b.txt"), scheduledEvent("call_a", "<cwd>/a.txt")],
    CONTEXT,
  );
  assert.notDeepEqual(
    compareEventStream(scheduleLeft, scheduleRight),
    [],
    "调度顺序是行为，必须被比较",
  );
});
