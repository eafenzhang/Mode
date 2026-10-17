// M2 Task 4 Step 0 探针：`Bash` 运行中收到取消请求时的行为。
//
// 用法：
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-bash-cancel.mts
//
// 纪律：只运行旧实现、观察它的输入输出与事件；不读实现体。取消方法名从协议**声明面**
// 得到候选（v1 的 `session/stop`），逐个试并把结果原样打印——探不到就写「未观察」。
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { startOracleRuntime } from "./driver.mjs";
import type { MockStep } from "./probe-support.mjs";

const script: MockStep[] = [
  {
    toolCalls: [
      {
        name: "Bash",
        input: { command: "sleep 30", timeout: 600000, description: "Sleep 30s" },
      },
    ],
  },
  { text: "after cancel step" },
];

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

async function drainFor(
  events: AsyncIterable<unknown>,
  timeoutMs: number,
): Promise<"closed" | "timeout"> {
  const timer = new Promise<"timeout">((resolve) => setTimeout(() => resolve("timeout"), timeoutMs));
  const consuming = (async () => {
    for await (const _event of events) {
      // 只等终态；事件本身保存在 session.frames 里。
    }
    return "closed" as const;
  })();
  return Promise.race([consuming, timer]);
}

const dataRoot = await mkdtemp(join(tmpdir(), "mode-probe-"));
const cwd = await mkdtemp(join(tmpdir(), "mode-probe-ws-"));
const session = await startOracleRuntime({ dataRoot, cwd, script });
try {
  await session.send("cancel probe");
  const sawStart = await waitForToolStart(session.frames, 10_000);
  process.stdout.write(`\n########## cancel：工具已启动 = ${sawStart} ##########\n`);

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

  const outcome = await drainFor(session.events, 15_000);
  process.stdout.write(`\n########## cancel：等待终态 = ${outcome} ##########\n`);
  process.stdout.write(`\n########## cancel：相关帧（原样）##########\n`);
  for (const frame of session.frames) {
    if (!relevant(frame)) continue;
    process.stdout.write(`${JSON.stringify(frame)}\n`);
  }
  process.stdout.write(`\n########## cancel：stderr ##########\n${session.stderr}\n`);
  // 取消没生效时后台仍有一个 turn；`stop()` 会关闭事件队列并杀掉进程，所以这里不再排空。
} finally {
  await session.stop();
  await rm(dataRoot, { recursive: true, force: true });
  await rm(cwd, { recursive: true, force: true });
}
