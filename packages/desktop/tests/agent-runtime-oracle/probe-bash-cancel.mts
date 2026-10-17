// M2 Task 4 Step 0 探针：`Bash` 运行中收到取消请求时的行为。
//
// 用法：
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-bash-cancel.mts
//
// 纪律：只运行旧实现、观察它的输入输出与事件；不读实现体。取消方法名从协议**声明面**
// 得到候选（v1 的 `session/stop`），逐个试并把结果原样打印——探不到就写「未观察」。
import { execFileSync } from "node:child_process";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { startOracleRuntime } from "./driver.mjs";
import type { MockStep } from "./probe-support.mjs";

const script: MockStep[] = [
  {
    toolCalls: [
      {
        name: "Bash",
        // 把孙进程的 pid 留到磁盘上，取消后直接查它是否存活（与超时路径同一口径）。
        input: {
          command: "sleep 30 & echo $! > child.pid; wait",
          timeout: 600000,
          description: "Sleep 30s",
        },
      },
    ],
  },
  { text: "after cancel step" },
];

/** 用 bash 的 `kill -0` 查 MSYS pid（Node 的 process.kill 用的是 Windows pid，口径不同）。 */
function checkGrandchild(pid: string, cwd: string): string {
  if (!/^\d+$/.test(pid)) return `pid 非法：${JSON.stringify(pid)}`;
  try {
    const output = execFileSync("bash", ["-lc", `kill -0 ${pid} 2>/dev/null && echo alive || echo dead`], {
      cwd,
      encoding: "utf8",
    });
    return output.trim();
  } catch (error) {
    return `check failed: ${String(error)}`;
  }
}

function relevant(frame: { type: string; payload: unknown }): boolean {
  return (
    frame.type.startsWith("tool.") ||
    frame.type.startsWith("turn.") ||
    frame.type.startsWith("permission.") ||
    frame.type === "session.updated"
  );
}

async function waitForToolStart(
  frames: readonly { type: string; payload: unknown }[],
  timeoutMs: number,
): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const started = frames.some((frame) => {
      if (frame.type !== "tool.updated") return false;
      const payload = frame.payload as { kind?: string } | undefined;
      return payload?.kind === "scheduled" || payload?.kind === "started";
    });
    if (started) return true;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  return false;
}

/** 轮询 `frames` 等终态：事件已被前面的 `waitForToolStart` 消费过，直接读录制更可靠。 */
async function waitForTerminal(
  frames: readonly { type: string }[],
  timeoutMs: number,
): Promise<string> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const terminal = frames.find(
      (frame) => frame.type === "turn.completed" || frame.type === "turn.failed",
    );
    if (terminal) return terminal.type;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  return "timeout";
}

const dataRoot = await mkdtemp(join(tmpdir(), "mode-probe-"));
const cwd = await mkdtemp(join(tmpdir(), "mode-probe-ws-"));
const session = await startOracleRuntime({ dataRoot, cwd, script });
try {
  await session.send("cancel probe");
  const sawStart = await waitForToolStart(session.frames, 10_000);
  process.stdout.write(`\n########## cancel：工具已启动 = ${sawStart} ##########\n`);
  // 等脚本真的把孙进程 pid 写进 child.pid 再取消（否则取消可能发生在 shell 启动前，
  // 观察不到「已启动的孙进程是否被清理」）。
  let pidReady = false;
  const pidDeadline = Date.now() + 10_000;
  while (Date.now() < pidDeadline) {
    try {
      await readFile(join(cwd, "child.pid"));
      pidReady = true;
      break;
    } catch {
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
  }
  process.stdout.write(`\n########## cancel：child.pid 已落盘 = ${pidReady} ##########\n`);

  const attempts: unknown[] = [];
  const candidates: [string, unknown][] = [
    ["session/stop", { sessionId: session.sessionId }],
    ["session/stop", {}],
    ["session/interrupt", { sessionId: session.sessionId }],
  ];
  for (const [method, params] of candidates) {
    try {
      const result = await session.request(method, params);
      attempts.push({ method, params, result });
      break;
    } catch (error) {
      attempts.push({ method, params, error: String(error) });
    }
  }
  process.stdout.write(`\n########## cancel：取消请求尝试 ##########\n`);
  process.stdout.write(`${JSON.stringify(attempts, null, 2)}\n`);

  const outcome = await waitForTerminal(session.frames, 15_000);
  process.stdout.write(`\n########## cancel：终态事件 = ${outcome} ##########\n`);
  process.stdout.write(`\n########## cancel：相关帧（原样）##########\n`);
  for (const frame of session.frames) {
    if (!relevant(frame)) continue;
    process.stdout.write(`${JSON.stringify(frame)}\n`);
  }

  process.stdout.write(`\n########## cancel：孙进程是否被清理 ##########\n`);
  let pid = "";
  try {
    pid = (await readFile(join(cwd, "child.pid"), "utf8")).trim();
    process.stdout.write(`child.pid = ${pid}\n`);
  } catch (error) {
    process.stdout.write(`child.pid 读取失败：${String(error)}\n`);
  }
  let elapsedMs = 0;
  for (const targetMs of [0, 1_000, 2_000]) {
    const waitMs = targetMs - elapsedMs;
    if (waitMs > 0) await new Promise((resolve) => setTimeout(resolve, waitMs));
    elapsedMs = targetMs;
    process.stdout.write(`+${targetMs}ms: ${pid ? checkGrandchild(pid, cwd) : "(无 pid)"}\n`);
  }

  process.stdout.write(`\n########## cancel：stderr ##########\n${session.stderr}\n`);
  // 取消没生效时后台仍有一个 turn；`stop()` 会关闭事件队列并杀掉进程，所以这里不再排空。
} finally {
  await session.stop();
  await rm(dataRoot, { recursive: true, force: true });
  await rm(cwd, { recursive: true, force: true });
}
