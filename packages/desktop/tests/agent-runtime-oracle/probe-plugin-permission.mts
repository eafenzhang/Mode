// M3 插件宿主 Task 0.4：有副作用的插件/MCP 工具是否触发审批。
//
// 用法：
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-plugin-permission.mts
//
// 默认环境已带 `mcp__node_repl__js`（MCP 工具）。只观察 permission 事件与结果，不读实现体。
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { startOracleRuntime } from "./driver.mjs";

const dataRoot = await mkdtemp(join(tmpdir(), "mode-probe-plugin-perm-"));
const cwd = await mkdtemp(join(tmpdir(), "mode-probe-plugin-perm-ws-"));
const frames: Record<string, unknown>[] = [];
const session = await startOracleRuntime({
  dataRoot,
  cwd,
  script: [
    {
      toolCalls: [
        {
          name: "mcp__node_repl__js",
          input: { code: "1 + 1", title: "Probe node repl" },
        },
      ],
    },
    { text: "plugin permission probes done" },
  ],
  onRawFrame: (frame) => frames.push(frame),
});

function observed(frame: Record<string, unknown>) {
  if (frame.method !== "session/event") return undefined;
  const params = (frame.params ?? {}) as Record<string, unknown>;
  const type = typeof params.type === "string" ? params.type : undefined;
  const payload =
    typeof params.payload === "object" && params.payload !== null
      ? (params.payload as Record<string, unknown>)
      : {};
  return type ? { type, payload } : undefined;
}

try {
  await session.send("probe plugin permission");
  const deadline = Date.now() + 60_000;
  while (
    !frames.some((frame) => {
      const event = observed(frame);
      return event?.type === "turn.completed" || event?.type === "turn.failed";
    })
  ) {
    if (Date.now() > deadline) throw new Error("probe：没有终态");
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
} finally {
  await session.stop();
}

process.stdout.write("\n########## mcp__node_repl__js 的审批与结果 ##########\n");
for (const frame of frames) {
  const event = observed(frame);
  if (!event) continue;
  if (event.type === "tool.updated" && event.payload.kind === "started") {
    process.stdout.write(
      `started tool=${String(event.payload.toolName)} readOnly=${String(
        event.payload.readOnly,
      )} sideEffectScope=${String(event.payload.sideEffectScope)}\n`,
    );
  }
  if (event.type === "permission.requested") {
    process.stdout.write(
      `permission.requested tool=${String(event.payload.toolName)} risk=${String(
        event.payload.riskLevel,
      )} reason=${String(event.payload.reason)}\n`,
    );
  }
  if (
    event.type === "tool.updated" &&
    (event.payload.kind === "result" || event.payload.kind === "error")
  ) {
    process.stdout.write(
      `${event.payload.kind} ${String(event.payload.toolCallId)}: ${JSON.stringify(
        event.payload.error ?? event.payload.result,
      )}\n`,
    );
  }
}

await rm(dataRoot, { recursive: true, force: true });
await rm(cwd, { recursive: true, force: true });
