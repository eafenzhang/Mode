// M3 插件宿主 Task 0 探针：插件/MCP/Skill 工具面与错误形状。
//
// 用法：
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-plugin-host.mts
//
// 纪律：只运行旧实现、观察请求体里的工具声明与调用结果，不读实现体。
// 默认环境已经带一个 MCP 工具（`mcp__node_repl__js`）以及 `Skill` / `Agent`，先观察它们。
import { mkdtemp, rm } from "node:fs/promises";
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

async function waitForTerminals(frames: RawFrame[], count: number, timeoutMs: number) {
  const deadline = Date.now() + timeoutMs;
  while (true) {
    const terminals = frames.filter((frame) => {
      const event = observed(frame);
      return event?.type === "turn.completed" || event?.type === "turn.failed";
    }).length;
    if (terminals >= count) return;
    if (Date.now() > deadline) throw new Error(`probe：${timeoutMs}ms 内只等到 ${terminals} 个终态`);
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
}

async function runCase(
  title: string,
  script: MockStep[],
  drive: (context: {
    session: Awaited<ReturnType<typeof startOracleRuntime>>;
    frames: RawFrame[];
    declarations: Map<string, unknown>;
    names: string[];
  }) => Promise<void>,
): Promise<void> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-probe-plugin-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-probe-plugin-ws-"));
  const frames: RawFrame[] = [];
  const declarations = new Map<string, unknown>();
  let names: string[] = [];
  const session = await startOracleRuntime({
    dataRoot,
    cwd,
    script,
    onMockRequestBody: (_path, body) => {
      // 收尾请求可能带空 tools；只取第一份非空清单，别把它覆盖成 0。
      if (!Array.isArray(body.tools) || body.tools.length === 0) return;
      if (names.length === 0) {
        // 就地 push，保持 drive 拿到的引用可见（重新赋值不会更新已捕获的引用）。
        names.push(...body.tools.map((tool) => String((tool as { name?: unknown }).name ?? "")));
      }
      for (const tool of body.tools) {
        const record = tool as { name?: unknown };
        declarations.set(String(record.name), tool);
      }
    },
    onRawFrame: (frame) => frames.push(frame),
  });
  try {
    process.stdout.write(`\n########## ${title} ##########\n`);
    await drive({ session, frames, declarations, names });
  } finally {
    await session.stop();
    await rm(dataRoot, { recursive: true, force: true });
    await rm(cwd, { recursive: true, force: true });
  }
}

await runCase("插件/MCP/Skill 工具声明", [{ text: "plugin surface" }], async ({ session, frames, declarations, names }) => {
  await session.send("probe plugin surface");
  await waitForTerminals(frames, 1, 30_000);
  const interesting = names.filter(
    (name) => name.startsWith("mcp__") || ["Skill", "Agent"].includes(name),
  );
  process.stdout.write(`toolCount=${names.length}\n`);
  process.stdout.write(`plugin-ish tools=${JSON.stringify(interesting)}\n`);
  for (const name of interesting) {
    process.stdout.write(`\n--- ${name} ---\n${JSON.stringify(declarations.get(name), null, 2)}\n`);
  }
});

await runCase(
  "插件工具的错误形状",
  [
    { toolCalls: [{ name: "Skill", input: {} }] },
    { toolCalls: [{ name: "Skill", input: { skill: "definitely-not-a-skill" } }] },
    { toolCalls: [{ name: "mcp__nonexistent__tool", input: {} }] },
    { text: "plugin errors done" },
  ],
  async ({ session, frames }) => {
    await session.send("probe plugin errors");
    await waitForTerminals(frames, 1, 30_000);
    for (const frame of frames) {
      const event = observed(frame);
      if (!event) continue;
      if (event.type === "permission.requested") {
        process.stdout.write(`permission.requested tool=${String(event.payload.toolName)}\n`);
      }
      if (event.type === "tool.updated" && (event.payload.kind === "error" || event.payload.kind === "result")) {
        process.stdout.write(
          `${event.payload.kind} ${String(event.payload.toolCallId)}: ${JSON.stringify(
            event.payload.error ?? event.payload.result,
          )}\n`,
        );
      }
    }
  },
);
