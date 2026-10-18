// M3 插件宿主 Task 0.2：通过本地 marketplace 安装 fixture 插件，再验证运行时能发现它。
//
// 用法：
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-plugin-installed.mts
//
// 纪律：只运行 CLI / 旧运行时并观察行为；不读实现体。
import { execFileSync } from "node:child_process";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { startOracleRuntime } from "./driver.mjs";

const REPO_ROOT = fileURLToPath(new URL("../../../../", import.meta.url));
const TSX_CLI = join(REPO_ROOT, "node_modules", "tsx", "dist", "cli.mjs");
const CLI_ENTRY = join(REPO_ROOT, "apps", "mode-cli", "packages", "cli", "src", "main.ts");

const dataRoot = await mkdtemp(join(tmpdir(), "mode-probe-installed-"));
const cwd = await mkdtemp(join(tmpdir(), "mode-probe-installed-ws-"));
const marketplaceRoot = await mkdtemp(join(tmpdir(), "mode-probe-marketplace-"));
const env = {
  ...process.env,
  HOME: dataRoot,
  USERPROFILE: dataRoot,
  MODE_HOME: dataRoot,
  MODE_DATA_BASE_DIR: dataRoot,
};

function runCli(args: string[]): string {
  return execFileSync(process.execPath, [TSX_CLI, CLI_ENTRY, ...args], {
    env,
    encoding: "utf8",
    timeout: 60_000,
  });
}

// 1) 最小 marketplace + 一个 skill 插件。
const pluginRoot = join(marketplaceRoot, "plugins", "fixture");
await mkdir(join(pluginRoot, ".tutto-plugin"), { recursive: true });
await mkdir(join(pluginRoot, "skills", "fixture-skill"), { recursive: true });
await writeFile(
  join(pluginRoot, ".tutto-plugin", "plugin.json"),
  JSON.stringify({ name: "fixture", version: "1.0.0", description: "Fixture plugin" }, null, 2),
  "utf8",
);
await writeFile(
  join(pluginRoot, "skills", "fixture-skill", "SKILL.md"),
  [
    "---",
    "name: fixture-skill",
    "description: Fixture skill for plugin host observation",
    "---",
    "Say hello from the fixture skill.",
    "",
  ].join("\n"),
  "utf8",
);
await writeFile(
  join(marketplaceRoot, "marketplace.json"),
  JSON.stringify(
    {
      name: "fixture-marketplace",
      owner: { name: "Fixture" },
      plugins: [{ name: "fixture", source: "./plugins/fixture", description: "Fixture plugin" }],
    },
    null,
    2,
  ),
  "utf8",
);

process.stdout.write(`\n########## 安装 fixture 插件 ##########\n`);
process.stdout.write(runCli(["plugins", "marketplace", "add", marketplaceRoot, "--scope", "user"]));
process.stdout.write(runCli(["plugins", "install", "fixture@fixture-marketplace", "--scope", "user"]));

// 2) 用同一个数据根启动 oracle，验证 Skill 发现。
const frames: Record<string, unknown>[] = [];
const session = await startOracleRuntime({
  dataRoot,
  cwd,
  script: [
    { toolCalls: [{ name: "Skill", input: { skill: "fixture:fixture-skill" } }] },
    { toolCalls: [{ name: "Skill", input: { skill: "fixture-skill" } }] },
    { text: "installed fixture probes done" },
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
  await session.send("probe installed fixture");
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

await rm(dataRoot, { recursive: true, force: true });
await rm(cwd, { recursive: true, force: true });
await rm(marketplaceRoot, { recursive: true, force: true });
