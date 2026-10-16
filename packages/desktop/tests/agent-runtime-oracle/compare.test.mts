import { test } from "node:test";
import assert from "node:assert/strict";

import {
  compareEventStream,
  compareFileSnapshot,
  compareToolCalls,
  formatDiffs,
} from "./compare.mjs";
import { foldUnorderedRuns, type ToolCall } from "./normalize.mjs";

// 比较器是这一阶段的验收仪器：一个恒真的比较器会让「行为对拍通过」变成一句空话。
// 这一组用例只针对比较器本身（纯函数，不跑 oracle），钉住「差异必须被报出来」。

const readCall: ToolCall = {
  name: "Read",
  input: { file_path: "<cwd>/hello.txt" },
  ok: true,
  output: "hello",
};

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

test("无序组：连续同类型的 session.updated 顺序差异被折叠掉，其他类型不受影响", () => {
  const left = foldUnorderedRuns([
    { type: "session.updated", payload: { a: 1 } },
    { type: "session.updated", payload: { b: 2 } },
    { type: "turn.started", payload: {} },
  ]);
  const right = foldUnorderedRuns([
    { type: "session.updated", payload: { b: 2 } },
    { type: "session.updated", payload: { a: 1 } },
    { type: "turn.started", payload: {} },
  ]);
  assert.deepEqual(compareEventStream(left, right), []);

  const reorderedTool = foldUnorderedRuns([
    { type: "tool.updated", payload: { a: 1 } },
    { type: "tool.updated", payload: { b: 2 } },
  ]);
  const reorderedToolExpected = foldUnorderedRuns([
    { type: "tool.updated", payload: { b: 2 } },
    { type: "tool.updated", payload: { a: 1 } },
  ]);
  assert.notDeepEqual(
    compareEventStream(reorderedTool, reorderedToolExpected),
    [],
    "工具事件不在无序白名单里，顺序必须被比较",
  );
});
