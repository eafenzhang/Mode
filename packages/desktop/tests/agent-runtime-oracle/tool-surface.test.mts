// M2 Task 2：**工具面**的对拍用例（模型侧看到的工具清单）。
//
// 存在理由（复评 Minor 8）：`docs/specs/tauri-agent-runtime.md` §5 与 driver 的 `extraCliArgs`
// 都声称「那 21 条不随 `--surface` / `--mode` / `--disallowed-tools` 变化」，但此前只有
// **临时手跑**的观察，仓库里没有可复现的产物。这条用例把它变成自动的：
//
//   node scripts/rust-cargo.ts 之外：pnpm exec tsx --test packages/desktop/tests/agent-runtime-oracle/tool-surface.test.mts
//
// 判据（**没有手写的工具清单**）：两次会话声明的名字集合**必须相等**，且
// - `Bash` 在两次里**都被声明**（正对照：`--disallowed-tools Bash` 若真的影响会话面工具表，
//   它就该消失——这正是我们要否证的那条假设）；
// - `Read` 在（M2 只读切片里唯一注册的工具）；
// - `Glob` / `Grep` 不在（它们在会话面没有注册，见 `absence` 探针与夹具
//   `read-absent-search-tools.json`）。
import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { startOracleRuntime } from "./driver.mjs";

/** 起一个会话，返回**第一次模型请求**里声明的工具名（那一次就是主 agent 的真实清单）。 */
async function declaredToolNames(extraCliArgs: readonly string[]): Promise<string[]> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-oracle-tool-surface-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-oracle-tool-surface-ws-"));
  const lists: string[][] = [];
  const session = await startOracleRuntime({
    dataRoot,
    cwd,
    extraCliArgs,
    script: [
      { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/x.txt" } }] },
      { text: "ok" },
    ],
    onMockRequestBody: (_path, body) => {
      const tools = (body as { tools?: { name?: string }[] }).tools;
      if (!Array.isArray(tools)) return;
      lists.push(tools.map((tool) => String(tool.name ?? "")));
    },
  });
  try {
    await session.send("看一眼");
    // 跑到终态：保证第一次请求真的发生过（否则 lists 可能是空的，「相等」就没有意义）。
    for await (const event of session.events) {
      if (event.type === "turn.completed" || event.type === "turn.failed") break;
    }
    assert.ok(lists.length > 0, "没有观察到任何模型请求，这条用例没验到东西");
    return lists[0] ?? [];
  } finally {
    await session.stop();
    await rm(dataRoot, { recursive: true, force: true });
    await rm(cwd, { recursive: true, force: true });
  }
}

test("工具清单不随 CLI 参数变化（--surface/--mode/--disallowed-tools 的合取）", async () => {
  const baseline = await declaredToolNames([]);
  const withFlags = await declaredToolNames([
    "--surface",
    "desktop",
    "--mode",
    "build",
    "--disallowed-tools",
    "Bash",
  ]);

  assert.deepEqual(
    withFlags,
    baseline,
    "CLI 参数改变了会话面声明的工具清单——driver 的 extraCliArgs 注释与 spec §5 的口径都要改",
  );
  // 正对照：`--disallowed-tools Bash` 下 `Bash` 照样被声明 ⇒ 这些参数不影响会话面工具表。
  assert.ok(baseline.includes("Bash"), `基线里应当有 Bash，实收：${baseline.join(",")}`);
  assert.ok(
    withFlags.includes("Bash"),
    `带 --disallowed-tools Bash 时仍有 Bash，实收：${withFlags.join(",")}`,
  );
  // M2 只读切片：`Read` 在，`Glob`/`Grep` 不在。
  assert.ok(withFlags.includes("Read"), "Read 应当被声明");
  assert.ok(!withFlags.includes("Glob"), "Glob 不该被声明（未注册）");
  assert.ok(!withFlags.includes("Grep"), "Grep 不该被声明（未注册）");
});
