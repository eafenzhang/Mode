// M2 Task 4b 探针：`TaskOutput` / `TaskStop` 的声明面、错误形状与成功路径（黑盒观察）。
//
// 用法：
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-task-control.mts
//
// 纪律：只运行旧实现、观察输入输出与帧，不读实现体。
// 任务 id 只有在运行时才产生，所以成功路径用「由请求内容决定工具调用」的动态剧本步骤
// （见 mock-provider 的 `MockStep` 函数形态）；拿不到 id 时不猜。
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { startOracleRuntime, type OracleSession } from "./driver.mjs";
import type { MockStep } from "./mock-provider.mjs";

type RawFrame = Record<string, unknown>;

type CaseContext = {
  session: OracleSession;
  frames: RawFrame[];
  declarations: Map<string, unknown>;
};

function eventType(frame: RawFrame): string | undefined {
  if (frame.method !== "session/event") return undefined;
  const params = (frame.params ?? {}) as Record<string, unknown>;
  return typeof params.type === "string" ? params.type : undefined;
}

function eventPayload(frame: RawFrame): Record<string, unknown> | undefined {
  const params = (frame.params ?? {}) as Record<string, unknown>;
  const payload = params.payload;
  return typeof payload === "object" && payload !== null
    ? (payload as Record<string, unknown>)
    : undefined;
}

/** 最后一条 user 消息的文本（动态剧本据此决定下一步）。 */
function lastUserText(body: { messages?: { role?: string; content?: unknown }[] }): string {
  const messages = Array.isArray(body.messages) ? body.messages : [];
  const last = [...messages].reverse().find((message) => message.role === "user");
  return JSON.stringify(last?.content ?? "");
}

function taskIdFromNotification(body: { messages?: { role?: string; content?: unknown }[] }): string | undefined {
  const match = /<task-id>([^<]+)<\/task-id>/.exec(lastUserText(body));
  return match?.[1];
}

function terminalCount(frames: readonly RawFrame[]): number {
  return frames.filter((frame) => {
    const type = eventType(frame);
    return type === "turn.completed" || type === "turn.failed";
  }).length;
}

async function waitForTerminals(
  frames: readonly RawFrame[],
  count: number,
  timeoutMs: number,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (terminalCount(frames) < count) {
    if (Date.now() > deadline) {
      throw new Error(`probe：${timeoutMs}ms 内只等到 ${terminalCount(frames)} 个终态（期望 ${count}）`);
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
}

/** `toolCallId → toolName`：result/error 载荷自身不带 toolName（既有观察），只能从 scheduled 反查。 */
function toolNamesById(frames: readonly RawFrame[]): Map<string, string> {
  const names = new Map<string, string>();
  for (const frame of frames) {
    if (eventType(frame) !== "tool.updated") continue;
    const payload = eventPayload(frame);
    if (payload?.kind !== "scheduled") continue;
    names.set(String(payload.toolCallId), String(payload.toolName));
  }
  return names;
}

/** 打印调度/启动帧里与工具声明相关的字段（`readOnly` / `sideEffectScope` / 并行性）。 */
function printLifecycle(frames: readonly RawFrame[], toolName: string): void {
  const names = toolNamesById(frames);
  for (const frame of frames) {
    if (eventType(frame) !== "tool.updated") continue;
    const payload = eventPayload(frame);
    if (payload?.kind !== "scheduled" && payload?.kind !== "started") continue;
    if (names.get(String(payload.toolCallId)) !== toolName) continue;
    const keys: Record<string, unknown> = { kind: payload.kind };
    for (const key of ["canRunParallel", "parallelGroupIndex", "readOnly", "sideEffectScope"]) {
      if (payload[key] !== undefined) keys[key] = payload[key];
    }
    process.stdout.write(`${toolName} ${JSON.stringify(keys)}\n`);
  }
}

/** 打印指定工具名的 result/error 帧（`scheduled`/`started` 不打印）。 */
function printToolFrames(frames: readonly RawFrame[], toolName: string): void {
  const names = toolNamesById(frames);
  let found = false;
  for (const frame of frames) {
    if (eventType(frame) !== "tool.updated") continue;
    const payload = eventPayload(frame);
    if (payload?.kind !== "result" && payload?.kind !== "error") continue;
    if (names.get(String(payload.toolCallId)) !== toolName) continue;
    found = true;
    process.stdout.write(`${toolName}: ${JSON.stringify(payload)}\n`);
  }
  if (!found) process.stdout.write(`(${toolName} 没有 result/error 帧)\n`);
}

async function runCase(
  title: string,
  script: MockStep[],
  drive: (context: CaseContext) => Promise<void>,
): Promise<void> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-probe-tc-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-probe-tc-ws-"));
  const frames: RawFrame[] = [];
  const declarations = new Map<string, unknown>();
  const session = await startOracleRuntime({
    dataRoot,
    cwd,
    script,
    onMockRequestBody: (_path, body) => {
      if (!Array.isArray(body.tools)) return;
      for (const tool of body.tools) {
        const record = tool as { name?: unknown };
        if (record.name === "TaskOutput" || record.name === "TaskStop") {
          declarations.set(String(record.name), tool);
        }
      }
    },
    onRawFrame: (frame) => {
      frames.push(frame);
    },
  });
  try {
    process.stdout.write(`\n########## ${title} ##########\n`);
    await drive({ session, frames, declarations });
  } finally {
    await session.stop();
    await rm(dataRoot, { recursive: true, force: true });
    await rm(cwd, { recursive: true, force: true });
  }
}

// 1) 声明面 + 错误形状：不存在的 id。
await runCase(
  "声明面与不存在的 task id",
  [
    { toolCalls: [{ name: "TaskOutput", input: { task_id: "exec_does_not_exist" } }] },
    { toolCalls: [{ name: "TaskStop", input: { task_id: "exec_does_not_exist" } }] },
    { text: "probe task-control done" },
  ],
  async ({ session, frames, declarations }) => {
    await session.send("probe missing task id");
    await waitForTerminals(frames, 1, 30_000);
    for (const name of ["TaskOutput", "TaskStop"]) {
      const declaration = declarations.get(name);
      process.stdout.write(
        declaration === undefined
          ? `${name}: (未在请求体里出现)\n`
          : `${name}: ${JSON.stringify(declaration, null, 2)}\n`,
      );
    }
    process.stdout.write("\n-- 调度/启动字段 --\n");
    printLifecycle(frames, "TaskOutput");
    printLifecycle(frames, "TaskStop");
    process.stdout.write("\n-- 错误形状 --\n");
    printToolFrames(frames, "TaskOutput");
    printToolFrames(frames, "TaskStop");
  },
);

// 1b) 缺 task_id：两个工具的入参校验形状。
await runCase(
  "缺少 task_id",
  [
    { toolCalls: [{ name: "TaskOutput", input: {} }] },
    { toolCalls: [{ name: "TaskStop", input: {} }] },
    { text: "probe missing input done" },
  ],
  async ({ session, frames }) => {
    await session.send("probe missing task id input");
    await waitForTerminals(frames, 1, 30_000);
    process.stdout.write("\n-- 缺参调度/启动字段 --\n");
    printLifecycle(frames, "TaskOutput");
    printLifecycle(frames, "TaskStop");
    process.stdout.write("\n-- 缺参错误形状 --\n");
    printToolFrames(frames, "TaskOutput");
    printToolFrames(frames, "TaskStop");
  },
);

// 2) TaskOutput 成功路径：后台任务完成 → 通知 turn 里用真实 id 调 TaskOutput。
await runCase(
  "TaskOutput 成功路径",
  [
    {
      toolCalls: [
        {
          name: "Bash",
          input: {
            command: "printf 'quick\\n'; sleep 1; printf 'quick-done\\n'",
            run_in_background: true,
            description: "Quick background",
          },
        },
      ],
    },
    { text: "turn one done" },
    (body) => {
      const taskId = taskIdFromNotification(body);
      return taskId
        ? { toolCalls: [{ name: "TaskOutput", input: { task_id: taskId, block: true, timeout: 30_000 } }] }
        : { text: "no task notification" };
    },
    { text: "task output turn done" },
  ],
  async ({ session, frames }) => {
    await session.send("start quick background");
    // 第 1 个终态是首 turn；第 2 个是通知 turn（内含 TaskOutput）。
    await waitForTerminals(frames, 2, 45_000);
    process.stdout.write("\n-- TaskOutput 调度/启动字段 --\n");
    printLifecycle(frames, "TaskOutput");
    process.stdout.write("\n-- TaskOutput 结果 --\n");
    printToolFrames(frames, "TaskOutput");
  },
);

// 2b) TaskOutput 失败任务：非零退出的后台命令，通知 turn 里取输出。
await runCase(
  "TaskOutput 失败任务",
  [
    {
      toolCalls: [
        {
          name: "Bash",
          input: {
            command: "printf 'bad\\n'; exit 7",
            run_in_background: true,
            description: "Failing background",
          },
        },
      ],
    },
    { text: "turn one done" },
    (body) => {
      const taskId = taskIdFromNotification(body);
      return taskId
        ? { toolCalls: [{ name: "TaskOutput", input: { task_id: taskId, block: true, timeout: 30_000 } }] }
        : { text: "no task notification" };
    },
    { text: "failed output turn done" },
  ],
  async ({ session, frames }) => {
    await session.send("start failing background");
    await waitForTerminals(frames, 2, 45_000);
    process.stdout.write("\n-- 失败任务的 TaskOutput --\n");
    printToolFrames(frames, "TaskOutput");
  },
);

// 3) TaskStop 成功路径：抓到真实 id 后，用第二次用户输入让模型停掉长任务。
let capturedTaskId: string | undefined;
await runCase(
  "TaskStop 成功路径",
  [
    {
      toolCalls: [
        {
          name: "Bash",
          input: {
            command: "sleep 30",
            run_in_background: true,
            description: "Long background",
          },
        },
      ],
    },
    { text: "turn one done" },
    (body) => {
      if (lastUserText(body).includes("stop it") && capturedTaskId) {
        return { toolCalls: [{ name: "TaskStop", input: { task_id: capturedTaskId } }] };
      }
      return { text: "nothing to stop" };
    },
    { text: "stop turn done" },
    // 通知 turn（status=killed）走这一步；它不请求输出。
    { text: "killed notification turn done" },
    // 收到 "output it" 后用同一个 id 调 TaskOutput，观察 cancelled 状态的返回形状。
    (body) => {
      if (lastUserText(body).includes("output it") && capturedTaskId) {
        return {
          toolCalls: [
            { name: "TaskOutput", input: { task_id: capturedTaskId, block: true, timeout: 5000 } },
          ],
        };
      }
      return { text: "no output request" };
    },
    { text: "output-after-stop done" },
  ],
  async ({ session, frames }) => {
    await session.send("start long background");
    await waitForTerminals(frames, 1, 30_000);

    for (const frame of frames) {
      if (eventType(frame) !== "tool.updated") continue;
      const payload = eventPayload(frame);
      if (payload?.kind !== "result") continue;
      const match = /exec_[0-9a-f-]+/.exec(JSON.stringify(payload.result ?? {}));
      if (match) {
        capturedTaskId = match[0];
        break;
      }
    }
    process.stdout.write(`\n捕获到的 task id: ${capturedTaskId ?? "(未捕获)"}\n`);
    if (!capturedTaskId) return;

    await session.send("stop it");
    await waitForTerminals(frames, 2, 30_000);
    process.stdout.write("\n-- TaskStop 结果 --\n");
    printToolFrames(frames, "TaskStop");
    process.stdout.write("\n-- TaskStop 之后的帧 --\n");
    for (const frame of frames.slice(frames.findIndex((item) => eventType(item) === "turn.completed"))) {
      const type = eventType(frame);
      if (type === undefined) continue;
      process.stdout.write(`${type} ${JSON.stringify(eventPayload(frame) ?? {})}\n`);
    }

    // 停掉的任务会不会像自然结束那样再触发一个通知 turn？等 2s 看新增帧。
    const lengthAtStop = frames.length;
    await new Promise((resolve) => setTimeout(resolve, 2_000));
    process.stdout.write("\n-- stop 后 2s 内新增帧 --\n");
    const added = frames.slice(lengthAtStop);
    if (added.length === 0) process.stdout.write("(没有新增帧)\n");
    for (const frame of added) {
      const type = eventType(frame);
      if (type === undefined) continue;
      const payload = eventPayload(frame) ?? {};
      const input = typeof payload.input === "string" ? payload.input : undefined;
      process.stdout.write(`${type}${input ? ` input=${JSON.stringify(input)}` : ""}\n`);
    }

    // 等通知 turn 结束（否则 session/send 会撞上 -32010），再对已停止的任务取输出。
    await waitForTerminals(frames, 3, 30_000);
    await session.send("output it");
    await waitForTerminals(frames, 4, 30_000);
    process.stdout.write("\n-- 已停止任务的 TaskOutput --\n");
    printToolFrames(frames, "TaskOutput");
  },
);
