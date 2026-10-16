// M2 Task 0 Step 5：三层对拍比较器（spec §2）。
//
// **只有这一份实现**：Rust 侧用例不实现第二份比较器，它把自己投影成同一形状后做精确相等断言
// （两份规则表必然漂移，漂移的比较器会让对拍失去意义）。
//
// 输入约定：三层都必须是**已规范化**的观测（`normalize.mts` 的 `projectToolCalls` /
// `takeFileSnapshot` / `projectEventStream`）。比较器只做结构比较，不重复白名单规则。
import type { FileSnapshot, NormalizedEvent, ToolCall } from "./normalize.mjs";
import { stableStringify } from "./normalize.mjs";

export type Diff = {
  layer: "toolCalls" | "files" | "events";
  /** 差异位置：`toolCalls[1].input`、`files/hello.txt`、`events[7]`。 */
  path: string;
  message: string;
  actual: unknown;
  expected: unknown;
};

function toolCallDiffs(actual: ToolCall, expected: ToolCall, path: string): Diff[] {
  const diffs: Diff[] = [];
  if (actual.name !== expected.name) {
    diffs.push({
      layer: "toolCalls",
      path: `${path}.name`,
      message: "工具名不同",
      actual: actual.name,
      expected: expected.name,
    });
  }
  const actualInput = stableStringify(actual.input);
  const expectedInput = stableStringify(expected.input);
  if (actualInput !== expectedInput) {
    diffs.push({
      layer: "toolCalls",
      path: `${path}.input`,
      message: "工具入参不同",
      actual: actual.input,
      expected: expected.input,
    });
  }
  if (actual.ok !== expected.ok) {
    diffs.push({
      layer: "toolCalls",
      path: `${path}.ok`,
      message: "工具成败不同",
      actual: actual.ok,
      expected: expected.ok,
    });
  }
  if (actual.output !== expected.output) {
    diffs.push({
      layer: "toolCalls",
      path: `${path}.output`,
      message: "工具输出不同",
      actual: actual.output,
      expected: expected.output,
    });
  }
  return diffs;
}

/** 第一层：工具调用序列——逐项按序比较，长度不等即报差异。 */
export function compareToolCalls(actual: ToolCall[], expected: ToolCall[]): Diff[] {
  const diffs: Diff[] = [];
  if (actual.length !== expected.length) {
    diffs.push({
      layer: "toolCalls",
      path: "toolCalls.length",
      message: `工具调用条数不同（实际 ${actual.length}，期望 ${expected.length}）`,
      actual: actual.map((call) => call.name),
      expected: expected.map((call) => call.name),
    });
  }
  const shared = Math.min(actual.length, expected.length);
  for (let index = 0; index < shared; index += 1) {
    const actualCall = actual[index];
    const expectedCall = expected[index];
    if (!actualCall || !expectedCall) continue;
    diffs.push(...toolCallDiffs(actualCall, expectedCall, `toolCalls[${index}]`));
  }
  return diffs;
}

/** 第二层：文件快照——键集必须相同，逐键比内容哈希。 */
export function compareFileSnapshot(actual: FileSnapshot, expected: FileSnapshot): Diff[] {
  const diffs: Diff[] = [];
  for (const key of Object.keys(actual).sort()) {
    if (!(key in expected)) {
      diffs.push({
        layer: "files",
        path: `files/${key}`,
        message: "实际多出文件",
        actual: actual[key],
        expected: undefined,
      });
    }
  }
  for (const key of Object.keys(expected).sort()) {
    if (!(key in actual)) {
      diffs.push({
        layer: "files",
        path: `files/${key}`,
        message: "实际缺少文件",
        actual: undefined,
        expected: expected[key],
      });
      continue;
    }
    if (actual[key] !== expected[key]) {
      diffs.push({
        layer: "files",
        path: `files/${key}`,
        message: "文件内容哈希不同",
        actual: actual[key],
        expected: expected[key],
      });
    }
  }
  return diffs;
}

/** 第三层：事件流——按序逐条比较 type 与规范化后的 payload。 */
export function compareEventStream(actual: NormalizedEvent[], expected: NormalizedEvent[]): Diff[] {
  const diffs: Diff[] = [];
  if (actual.length !== expected.length) {
    diffs.push({
      layer: "events",
      path: "events.length",
      message: `事件条数不同（实际 ${actual.length}，期望 ${expected.length}）`,
      actual: actual.map((event) => event.type),
      expected: expected.map((event) => event.type),
    });
  }
  const shared = Math.min(actual.length, expected.length);
  for (let index = 0; index < shared; index += 1) {
    const actualEvent = actual[index];
    const expectedEvent = expected[index];
    if (!actualEvent || !expectedEvent) continue;
    if (actualEvent.type !== expectedEvent.type) {
      diffs.push({
        layer: "events",
        path: `events[${index}].type`,
        message: "事件类型不同",
        actual: actualEvent.type,
        expected: expectedEvent.type,
      });
      continue;
    }
    const actualPayload = stableStringify(actualEvent.payload);
    const expectedPayload = stableStringify(expectedEvent.payload);
    if (actualPayload !== expectedPayload) {
      diffs.push({
        layer: "events",
        path: `events[${index}].payload`,
        message: `事件 ${actualEvent.type} 的 payload 不同`,
        actual: actualEvent.payload,
        expected: expectedEvent.payload,
      });
    }
  }
  return diffs;
}

/** 三层一次比完；渲染成一行一行的差异，便于用例直接 `assert.deepEqual(diffs, [])`。 */
export function compareAll(
  actual: { toolCalls: ToolCall[]; files: FileSnapshot; events: NormalizedEvent[] },
  expected: { toolCalls: ToolCall[]; files: FileSnapshot; events: NormalizedEvent[] },
): Diff[] {
  return [
    ...compareToolCalls(actual.toolCalls, expected.toolCalls),
    ...compareFileSnapshot(actual.files, expected.files),
    ...compareEventStream(actual.events, expected.events),
  ];
}

export function formatDiffs(diffs: readonly Diff[]): string {
  return diffs
    .map(
      (diff) =>
        `- [${diff.layer}] ${diff.path}：${diff.message}\n  实际：${JSON.stringify(diff.actual)}\n  期望：${JSON.stringify(diff.expected)}`,
    )
    .join("\n");
}
