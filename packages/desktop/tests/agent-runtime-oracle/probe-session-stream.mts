// M2 Task 5 Step 0 探针：完整会话事件序列 + 后台通知 turn 的准入顺序。
//
// 用法：
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-session-stream.mts
//
// 纪律：只运行旧实现、观察帧序列，不读实现体。
// 三个用例：
//   1) 纯文本 turn 的事件序列（model.streaming / turn.completed 的逐字形状）；
//   2) 工具 turn 的事件序列（工具事件在模型流之间的位置）；
//   3) 后台任务完成时另一个 turn 正在运行——通知 turn 是排队还是穿插。
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { startOracleRuntime, type OracleSession } from "./driver.mjs";
import type { MockStep } from "./mock-provider.mjs";

type RawFrame = Record<string, unknown>;
type Captured = { frame: RawFrame; at: number };
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

/** 只打印这个问题要看的字段，避免整帧淹没输出。 */
function describe(event: Observed): string {
  const { type, payload } = event;
  if (type === "model.streaming") {
    const kind = String(payload.kind ?? "");
    if (kind === "text_delta") {
      return `${type} kind=${kind} text=${JSON.stringify(payload.delta ?? "")}`;
    }
    if (kind === "tool_call") {
      return `${type} kind=${kind} tool=${String(payload.toolName ?? "")} input=${JSON.stringify(payload.input)}`;
    }
    return `${type} kind=${kind} toolCallId=${String(payload.toolCallId ?? "")}`;
  }
  if (type === "turn.completed") {
    return `${type} resultType=${String(payload.resultType)} tokenCount=${String(payload.tokenCount)} usageKeys=${Object.keys((payload.usage ?? {}) as object).join(",")}`;
  }
  if (type === "turn.started") {
    const input = String(payload.input ?? "");
    const tag = input.startsWith("<task-notification>") ? " [task-notification]" : "";
    return `${type} turnNumber=${String(payload.turnNumber)} input=${JSON.stringify(input.slice(0, 80))}${tag}`;
  }
  if (type === "session.updated") {
    const keys = Object.keys(payload).filter((key) =>
      ["messageCount", "content", "toolCallCount", "stopReason", "status", "taskId"].includes(key),
    );
    return `${type} ${keys.map((key) => `${key}=${JSON.stringify(payload[key])}`).join(" ")}`;
  }
  if (type === "tool.updated") {
    // progress 是 Task 5 尚未镜像的 kind，整条打出来供建模。
    if (payload.kind === "progress") return `${type} kind=progress ${JSON.stringify(payload)}`;
    return `${type} kind=${String(payload.kind)} tool=${String(payload.toolName ?? "")}`;
  }
  return type;
}

function terminalCount(captured: readonly Captured[]): number {
  return captured.filter(({ frame }) => {
    const event = observed(frame);
    return event?.type === "turn.completed" || event?.type === "turn.failed";
  }).length;
}

async function waitForTerminals(
  captured: readonly Captured[],
  count: number,
  timeoutMs: number,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (terminalCount(captured) < count) {
    if (Date.now() > deadline) {
      throw new Error(`probe：${timeoutMs}ms 内只等到 ${terminalCount(captured)} 个终态`);
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
}

async function runCase(
  title: string,
  script: MockStep[],
  seed: Record<string, string>,
  drive: (context: { session: OracleSession; captured: Captured[] }) => Promise<void>,
): Promise<void> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-probe-stream-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-probe-stream-ws-"));
  for (const [path, content] of Object.entries(seed)) {
    await writeFile(join(cwd, path), content);
  }
  const captured: Captured[] = [];
  const session = await startOracleRuntime({
    dataRoot,
    cwd,
    script,
    onRawFrame: (frame) => {
      captured.push({ frame, at: Date.now() });
    },
  });
  try {
    process.stdout.write(`\n########## ${title} ##########\n`);
    await drive({ session, captured });
  } finally {
    await session.stop();
    await rm(dataRoot, { recursive: true, force: true });
    await rm(cwd, { recursive: true, force: true });
  }
}

function printTimeline(captured: readonly Captured[]): void {
  const start = captured[0]?.at ?? Date.now();
  for (const { frame, at } of captured) {
    const event = observed(frame);
    if (!event) continue;
    process.stdout.write(`+${String(at - start).padStart(5)}ms ${describe(event)}\n`);
  }
}

// 1) 纯文本 turn。
await runCase("纯文本 turn", [{ text: "hello from mock" }], {}, async ({ session, captured }) => {
  await session.send("probe text turn");
  await waitForTerminals(captured, 1, 30_000);
  printTimeline(captured);
});

// 2) 工具 turn（Read）。
await runCase(
  "工具 turn",
  [
    { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/hello.txt" } }] },
    { text: "tool turn done" },
  ],
  { "hello.txt": "hello\n" },
  async ({ session, captured }) => {
    await session.send("probe tool turn");
    await waitForTerminals(captured, 1, 30_000);
    printTimeline(captured);
  },
);

// 3) 后台任务完成 vs 运行中的 turn。
await runCase(
  "后台通知与运行中 turn",
  [
    {
      toolCalls: [
        {
          name: "Bash",
          input: {
            command: "sleep 2; printf 'bg-done\\n'",
            run_in_background: true,
            description: "Two second background",
          },
        },
      ],
    },
    { text: "turn one done" },
    {
      toolCalls: [
        {
          name: "Bash",
          input: { command: "sleep 6", timeout: 600000, description: "Long foreground" },
        },
      ],
    },
    { text: "turn two done" },
    { text: "notification handled" },
    { text: "fallback" },
  ],
  {},
  async ({ session, captured }) => {
    await session.send("start background");
    await waitForTerminals(captured, 1, 30_000);
    await session.send("run long foreground");
    // 等 turn2 与通知 turn 都结束（通知可能在 turn2 之后才开）。
    try {
      await waitForTerminals(captured, 3, 20_000);
    } catch (error) {
      process.stdout.write(`\n(等待第 3 个终态超时：${String(error)})\n`);
    }
    printTimeline(captured);
    const notification = captured.find(({ frame }) => {
      const event = observed(frame);
      return (
        event?.type === "turn.started" &&
        String(event.payload.input ?? "").startsWith("<task-notification>")
      );
    });
    const turnTwoCompletedIndex = captured.findIndex(({ frame }) => {
      const event = observed(frame);
      return event?.type === "turn.completed" && String(event.payload.response ?? "") === "turn two done";
    });
    const notificationIndex = notification ? captured.indexOf(notification) : -1;
    process.stdout.write(
      `\n判定：通知 turn.started 位于 turn2.completed ${notificationIndex > turnTwoCompletedIndex ? "之后（排队）" : "之前（穿插）"}\n`,
    );
  },
);
