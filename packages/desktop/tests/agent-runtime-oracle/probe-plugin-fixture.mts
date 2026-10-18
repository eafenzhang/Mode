// M3 插件宿主 Task 0.2：fixture 插件能否被隔离环境发现（黑盒观察）。
//
// 用法：
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-plugin-fixture.mts
//
// 在官方插件缓存根放一个最小插件（plugin.json + commands/hello.md），
// 然后调用 `Skill` 看它是否被命名空间发现。只观察行为，不读实现体。
import { mkdir, mkdtemp, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { startOracleRuntime } from "./driver.mjs";

const dataRoot = await mkdtemp(join(tmpdir(), "mode-probe-fixture-"));
const cwd = await mkdtemp(join(tmpdir(), "mode-probe-fixture-ws-"));
const pluginRoot = join(
  dataRoot,
  ".mode",
  "cli",
  "plugins",
  "cache",
  "mode-plugins-official",
  "fixture",
  "1.0.0",
);
await mkdir(join(pluginRoot, ".tutto-plugin"), { recursive: true });
await mkdir(join(pluginRoot, "commands"), { recursive: true });
await writeFile(
  join(pluginRoot, ".tutto-plugin", "plugin.json"),
  JSON.stringify({ name: "fixture", version: "1.0.0", description: "Fixture plugin" }, null, 2),
  "utf8",
);
await writeFile(
  join(pluginRoot, "commands", "hello.md"),
  ["---", "description: Fixture command", "---", "Say hello from the fixture plugin.", ""].join(
    "\n",
  ),
  "utf8",
);

const frames: Record<string, unknown>[] = [];
const session = await startOracleRuntime({
  dataRoot,
  cwd,
  script: [
    { toolCalls: [{ name: "Skill", input: { skill: "fixture:hello" } }] },
    { toolCalls: [{ name: "Skill", input: { skill: "hello" } }] },
    { text: "fixture probes done" },
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
  await session.send("probe fixture plugin");
  const deadline = Date.now() + 30_000;
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

process.stdout.write("\n########## fixture 插件的 Skill 调用 ##########\n");
for (const frame of frames) {
  const event = observed(frame);
  if (!event) continue;
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

process.stdout.write("\n########## 缓存目录（会话后）##########\n");
try {
  for (const entry of await readdir(
    join(dataRoot, ".mode", "cli", "plugins", "cache", "mode-plugins-official"),
    { withFileTypes: true },
  )) {
    process.stdout.write(`${entry.name}${entry.isDirectory() ? "/" : ""}\n`);
  }
} catch (error) {
  process.stdout.write(`读取缓存目录失败：${String(error)}\n`);
}

await rm(dataRoot, { recursive: true, force: true });
await rm(cwd, { recursive: true, force: true });
