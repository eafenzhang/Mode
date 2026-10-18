// M3 插件宿主 Task 0.1：隔离数据根里插件相关目录的落点（黑盒观察）。
//
// 用法：
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-plugin-roots.mts
//
// 只跑一次普通会话，然后把隔离根的两层目录列出来；不读实现体。
import { mkdtemp, readdir, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { startOracleRuntime } from "./driver.mjs";

const dataRoot = await mkdtemp(join(tmpdir(), "mode-probe-roots-"));
const cwd = await mkdtemp(join(tmpdir(), "mode-probe-roots-ws-"));
const session = await startOracleRuntime({
  dataRoot,
  cwd,
  script: [{ text: "roots probe" }],
});

try {
  await session.send("probe plugin roots");
  const deadline = Date.now() + 30_000;
  while (
    !session.frames.some(
      (frame) => frame.type === "turn.completed" || frame.type === "turn.failed",
    )
  ) {
    if (Date.now() > deadline) throw new Error("probe：没有终态");
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
} finally {
  await session.stop();
}

async function walk(root: string, depth: number, prefix = ""): Promise<void> {
  if (depth < 0) return;
  let entries: Awaited<ReturnType<typeof readdir>>;
  try {
    entries = await readdir(root, { withFileTypes: true });
  } catch {
    return;
  }
  for (const entry of entries) {
    if (entry.name === "node_modules") continue;
    const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
    const absolute = join(root, entry.name);
    const info = await stat(absolute).catch(() => undefined);
    process.stdout.write(`${relative}${entry.isDirectory() ? "/" : ""}\n`);
    if (entry.isDirectory()) await walk(absolute, depth - 1, relative);
    else if (info && info.size === 0) continue;
  }
}

process.stdout.write(`\n########## dataRoot=${dataRoot} ##########\n`);
await walk(dataRoot, 3);
process.stdout.write("\n########## 插件相关路径探测 ##########\n");
for (const relative of [
  ".mode/plugins",
  ".mode/cli/plugins",
  ".claude/plugins",
  ".agents/plugins",
  ".mode/plugins/installed_plugins.json",
]) {
  const target = join(dataRoot, relative);
  try {
    const info = await stat(target);
    process.stdout.write(`${relative}: ${info.isDirectory() ? "dir" : "file"}\n`);
  } catch {
    process.stdout.write(`${relative}: (不存在)\n`);
  }
}

await rm(dataRoot, { recursive: true, force: true });
await rm(cwd, { recursive: true, force: true });
