// M2 Task 5D：录一份 **投影后** 的会话流夹具（desktop-continuous）。
//
// 用法：
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/record-session-stream.mts
//
// 只保留 Rust 侧已经能产出的四类事件：turn.started / model.streaming / tool.updated /
// turn.completed；session.updated 与 streamRecovery.updated 不在投影里（差异已登记）。
// 投影规则与 Rust 侧 `stream_parity` 用例逐条对应。
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { startOracleRuntime } from "./driver.mjs";
import type { MockStep } from "./mock-provider.mjs";

type RawFrame = Record<string, unknown>;
type Observed = { type: string; payload: Record<string, unknown> };

function observed(frame: RawFrame): Observed | undefined {
  if (frame.method !== "session/event") return undefined;
  const params = (frame.params ?? {}) as Record<string, unknown>;
  const type = typeof params.type === "string" ? params.type : undefined;
  if (!type) return undefined;
  const payload =
    typeof params.payload === "object" && params.payload !== null
      ? (params.payload as Record<string, unknown>)
      : {};
  return { type, payload };
}

/** 把机器相关的 cwd 换回 `${cwd}`（夹具跨机器可回放）。 */
function normalizeValue(value: unknown, cwd: string): unknown {
  if (typeof value === "string") {
    // `tool_input_delta` 里是 **JSON 文本**，路径的反斜杠被转义过一次，两种形态都要换。
    const escapedCwd = JSON.stringify(cwd).slice(1, -1);
    return value.replaceAll(cwd, "${cwd}").replaceAll(escapedCwd, "${cwd}");
  }
  if (Array.isArray(value)) return value.map((item) => normalizeValue(item, cwd));
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value as Record<string, unknown>).map(([key, inner]) => [
        key,
        normalizeValue(inner, cwd),
      ]),
    );
  }
  return value;
}

/** 与 Rust 侧投影逐条对应：只保留四类事件的关键字段。 */
function project(event: Observed, cwd: string): Record<string, unknown> | undefined {
  const { type, payload } = event;
  if (type === "turn.started") {
    return { type, turnNumber: payload.turnNumber, input: payload.input };
  }
  if (type === "model.streaming") {
    const projected: Record<string, unknown> = { type, kind: payload.kind };
    if (payload.kind === "text_delta" || payload.kind === "tool_input_delta") {
      projected.delta = payload.delta;
    }
    if (payload.kind === "tool_call") projected.input = payload.input;
    if (payload.toolCallId !== undefined) projected.toolCallId = payload.toolCallId;
    if (payload.toolName !== undefined) projected.toolName = payload.toolName;
    return normalizeValue(projected, cwd) as Record<string, unknown>;
  }
  if (type === "tool.updated") {
    const projected: Record<string, unknown> = { type, kind: payload.kind };
    if (payload.toolName !== undefined) projected.toolName = payload.toolName;
    return projected;
  }
  if (type === "turn.completed") {
    const usage = (payload.usage ?? {}) as Record<string, unknown>;
    return {
      type,
      resultType: payload.resultType,
      tokenCount: payload.tokenCount,
      usage: {
        inputTokens: usage.inputTokens,
        outputTokens: usage.outputTokens,
        modelRequestCount: usage.modelRequestCount,
      },
    };
  }
  return undefined;
}

type Case = {
  name: string;
  prompt: string;
  seed: Record<string, string>;
  steps: MockStep[];
};

const cases: Case[] = [
  {
    name: "text",
    prompt: "probe text turn",
    seed: {},
    steps: [{ text: "hello from mock" }],
  },
  {
    name: "tool",
    prompt: "probe tool turn",
    seed: { "hello.txt": "hello\n" },
    steps: [
      { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/hello.txt" } }] },
      { text: "tool turn done" },
    ],
  },
];

const recorded: Record<string, unknown>[] = [];
for (const testCase of cases) {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-record-stream-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-record-stream-ws-"));
  for (const [path, content] of Object.entries(testCase.seed)) {
    await writeFile(join(cwd, path), content);
  }
  const frames: RawFrame[] = [];
  const session = await startOracleRuntime({
    dataRoot,
    cwd,
    script: testCase.steps,
    onRawFrame: (frame) => frames.push(frame),
  });
  try {
    await session.send(testCase.prompt);
    const deadline = Date.now() + 30_000;
    while (
      !frames.some((frame) => {
        const event = observed(frame);
        return event?.type === "turn.completed" || event?.type === "turn.failed";
      })
    ) {
      if (Date.now() > deadline) throw new Error(`record：${testCase.name} 没有终态`);
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    const expect = frames
      .map((frame) => {
        const event = observed(frame);
        return event ? project(event, cwd) : undefined;
      })
      .filter((event): event is Record<string, unknown> => event !== undefined);
    recorded.push({
      name: testCase.name,
      prompt: testCase.prompt,
      seed: testCase.seed,
      steps: testCase.steps,
      expect,
    });
  } finally {
    await session.stop();
    await rm(dataRoot, { recursive: true, force: true });
    await rm(cwd, { recursive: true, force: true });
  }
}

const fixture = {
  case: "session-stream",
  direction: "oracle",
  frame: "session-event",
  note: "M2 Task 5D 投影：只含 turn.started / model.streaming / tool.updated / turn.completed；session.updated 与 streamRecovery.updated 不在投影内。",
  cases: recorded,
};
const path = join(import.meta.dirname, "fixtures", "session-stream.json");
await writeFile(path, `${JSON.stringify(fixture, null, 2)}\n`, "utf8");
process.stdout.write(`wrote ${path}\n`);
