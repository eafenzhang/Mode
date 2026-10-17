// M2 Task 4b 探针：`run_in_background` 的返回形状与后续事件（黑盒观察）。
//
// 用法：
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-bash-background.mts
//
// 纪律（docs/specs/clean-room-policy.md）：只运行旧实现、观察它的输入输出与帧序列，
// **不读实现体**。Task 4b 的范围必须由这里的观察决定（超时/取消/退出码已由 probe-bash 观察）。
//
// 本探针刻意不用 `runGroup`：它在 `turn.completed` 就停，会漏掉后台命令结束后的帧。
import { mkdtemp, readFile, readdir, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { startOracleRuntime } from "./driver.mjs";

type RawFrame = Record<string, unknown>;

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

/** 只印与本次问题相关的字段，避免把整帧（含大载荷）倒进终端。 */
function describeFrame(frame: RawFrame): string {
  const type = eventType(frame) ?? String(frame.method ?? "<unknown>");
  const payload = eventPayload(frame);
  if (!payload) return type;
  const keys: Record<string, unknown> = {};
  for (const key of [
    "kind",
    "toolName",
    "toolCallId",
    "background",
    "taskId",
    "content",
    "success",
    "resultType",
    "input",
    "artifactPath",
  ]) {
    if (payload[key] !== undefined) keys[key] = payload[key];
  }
  if (payload.result && typeof payload.result === "object") {
    keys.result = payload.result;
  }
  if (payload.error && typeof payload.error === "object") {
    keys.error = payload.error;
  }
  return `${type} ${JSON.stringify(keys)}`;
}

const dataRoot = await mkdtemp(join(tmpdir(), "mode-probe-bg-"));
const cwd = await mkdtemp(join(tmpdir(), "mode-probe-bg-ws-"));
const frames: RawFrame[] = [];

const session = await startOracleRuntime({
  dataRoot,
  cwd,
  // 第二步给收尾文本，让 turn 正常结束；后台命令在 turn 结束后才完成。
  script: [
    {
      toolCalls: [
        {
          name: "Bash",
          input: {
            // 成功路径：先写 marker，2s 后结束。
            command:
              "printf 'bg-start\\n' >> bg-marker.txt; sleep 2; printf 'bg-done\\n' >> bg-marker.txt; printf 'bg-out\\n'; printf 'bg-err\\n' >&2",
            run_in_background: true,
            description: "Write a marker in the background",
          },
        },
        {
          name: "Bash",
          input: {
            // 非零退出：1s 后失败，用来观察通知里的 exit code 文案与日志内容。
            command: "sleep 1; printf 'bad-out\\n'; printf 'bad-err\\n' >&2; exit 7",
            run_in_background: true,
            description: "Fail in the background",
          },
        },
      ],
    },
    { text: "probe background done" },
  ],
  onRawFrame: (frame) => {
    frames.push(frame);
  },
});

try {
  await session.send("probe run_in_background");
  // 等到 turn.completed（最多 30s）。
  const turnDeadline = Date.now() + 30_000;
  while (!frames.some((frame) => eventType(frame) === "turn.completed")) {
    if (Date.now() > turnDeadline) throw new Error("probe：30s 内没有 turn.completed");
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  const completedIndex = frames.findIndex((frame) => eventType(frame) === "turn.completed");

  // 后台命令 3s 才结束；再等 6s，收集 turn 之后的帧。
  await new Promise((resolve) => setTimeout(resolve, 6_000));

  process.stdout.write("\n########## run_in_background ##########\n");
  process.stdout.write("\n== turn.completed 之前的工具帧 ==\n");
  for (const frame of frames.slice(0, completedIndex + 1)) {
    const type = eventType(frame);
    if (type === "tool.updated") process.stdout.write(`${describeFrame(frame)}\n`);
  }

  process.stdout.write("\n== turn.completed 之后的全部帧 ==\n");
  const after = frames.slice(completedIndex + 1);
  if (after.length === 0) process.stdout.write("(没有后续帧)\n");
  for (const frame of after) process.stdout.write(`${describeFrame(frame)}\n`);

  process.stdout.write("\n== 后台命令是否真的执行 ==\n");
  try {
    process.stdout.write(`bg-marker.txt: ${JSON.stringify(await readFile(join(cwd, "bg-marker.txt"), "utf8"))}\n`);
  } catch (error) {
    process.stdout.write(`bg-marker.txt 读取失败：${String(error)}\n`);
  }

  process.stdout.write("\n== 数据根落盘（.mode 根下）==\n");
  for (const relative of ["cli/exec", "cli/sessions", "cli/artifacts", "cli/log", "v2"]) {
    const target = join(dataRoot, ".mode", relative);
    try {
      const info = await stat(target);
      process.stdout.write(`${relative}: ${info.isDirectory() ? "dir" : "file"}\n`);
    } catch {
      process.stdout.write(`${relative}: (不存在)\n`);
    }
  }

  const execDir = join(dataRoot, ".mode", "cli", "exec");
  try {
    for (const sessionDir of await readdir(execDir, { withFileTypes: true })) {
      if (!sessionDir.isDirectory()) continue;
      const dir = join(execDir, sessionDir.name);
      const files = await readdir(dir);
      for (const file of files) {
        const content = await readFile(join(dir, file), "utf8");
        process.stdout.write(`exec/${sessionDir.name}/${file}: ${JSON.stringify(content)}\n`);
      }
    }
  } catch (error) {
    process.stdout.write(`exec 目录遍历失败：${String(error)}\n`);
  }
} finally {
  await session.stop();
  await rm(dataRoot, { recursive: true, force: true });
  await rm(cwd, { recursive: true, force: true });
}
