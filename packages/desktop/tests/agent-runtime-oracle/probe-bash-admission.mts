// M2 Task 4 Step 0 探针：一个 turn 运行中再发 `session/send` 时，第二条输入如何处理。
//
// 用法：
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-bash-admission.mts
//
// 纪律：只运行旧实现、观察它的输入输出与事件；不读实现体。CommandInbox 的准入顺序必须来自
// 这里的观察（排队 / 拒绝 / 合并），不能靠猜。
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
        input: { command: "sleep 10", timeout: 60000, description: "Sleep 10s" },
      },
    ],
  },
  { text: "after first turn" },
  { text: "after second input" },
];

function waitForToolStart(
  frames: readonly { type: string; payload: unknown }[],
  timeoutMs: number,
): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;
  return (async () => {
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
  })();
}

const dataRoot = await mkdtemp(join(tmpdir(), "mode-probe-"));
const cwd = await mkdtemp(join(tmpdir(), "mode-probe-ws-"));
const session = await startOracleRuntime({ dataRoot, cwd, script });
try {
  await session.request("session/send", { sessionId: session.sessionId, content: "first input" });
  const sawStart = await waitForToolStart(session.frames, 10_000);
  process.stdout.write(`\n########## admission：第一条已在运行 = ${sawStart} ##########\n`);

  let second: unknown;
  try {
    second = await session.request("session/send", {
      sessionId: session.sessionId,
      content: "second input",
    });
  } catch (error) {
    second = { error: String(error) };
  }
  process.stdout.write(`\n########## admission：第二条 send 的应答 ##########\n`);
  process.stdout.write(`${JSON.stringify(second, null, 2)}\n`);

  // 观察窗口：足够第一条 sleep 结束（10s）+ 后续 turn。
  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  process.stdout.write(`\n########## admission：turn / 队列相关帧（原样）##########\n`);
  for (const frame of session.frames) {
    if (!frame.type.startsWith("turn.") && frame.type !== "session.updated") continue;
    process.stdout.write(`${JSON.stringify(frame)}\n`);
  }
  process.stdout.write(`\n########## admission：stderr ##########\n${session.stderr}\n`);
} finally {
  await session.stop();
  await rm(dataRoot, { recursive: true, force: true });
  await rm(cwd, { recursive: true, force: true });
}
