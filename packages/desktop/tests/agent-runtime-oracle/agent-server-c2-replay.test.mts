// C2 Task 8：Rust `agent-server` 的 C2 夹具回放用例（`pnpm test:agent-runtime-oracle` 的成员）。
//
// 期望值来自 C2 oracle 行为夹具（`fixtures/c2/*.json`，probe-c2-*.mts 机器录制），
// 比较器是 `normalize.mts` / `session-lifecycle-fixtures.mts`（normalizeDeep）+
// `compare.mts` 的同一份实现；回放口径（登记缺省过滤、可比子集）见
// `replay-c2.mts` 文件头与各 driver。未登记的差异原样报红——那是本用例的产出。
//
// 前置条件：`cargo build -p agent-server`（缺失时 harness 自动构建一次）。
// T6/T7 的夹具对照不在这里重复：`context-usage-settings` / `fresh-install-ledger`
// 的快照/账本断言在 `crates/agent-server/tests/{snapshot,lifecycle}.rs`（单比较器
// 纪律：同一条夹具只在一层做精确比较，另一层引用）。
import { readdirSync } from "node:fs";
import { test } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";

import { formatDiffs } from "./compare.mjs";
// driver 模块先于注册表取用（replay-c2 不反向静态依赖 driver，避免 import 环）。
import { registerCompactCases } from "./replay-c2-compact.mjs";
import { registerMemoryCases } from "./replay-c2-memory.mjs";
import {
  C2_REPLAY_CASES,
  loadC2Fixture,
  type C2ReplayResult,
} from "./replay-c2.mjs";

registerCompactCases(C2_REPLAY_CASES);
registerMemoryCases(C2_REPLAY_CASES);

/**
 * 已知 C2 覆盖清单：本套件逐条断言的 case（registry 注册面）。
 * 新增/减少都要显式更新本清单，不能静默吞掉。
 */
const COVERED_CASES = [
  "compact-manual",
  "compact-no-instructions",
  "compact-expected-revision",
  "compact-same-tick",
  "compact-provider-tool-call",
  "compact-provider-empty",
  "memory-recall",
  "memory-recall:disabled",
  "memory-recall:fresh-disabled",
] as const;

/**
 * 登记未覆盖（`tauri-agent-runtime.md` §C2 / `agent-c2-contract.md` 各节）：
 * - `compact-while-turn-running`：需要钉住「turn 执行中」窗口，TS 驱动无法确定性
 *   构造；`-32010` 拒绝形状由 Rust 单测断言
 *   （`crates/agent-server/tests/compact.rs` 的
 *   `compact_while_prompt_running_is_rejected_with_zero_side_effects`：门控 provider
 *   把 prompt 钉在 Running 相位，断言 -32010 文案、零落库、prompt 照常完成）。
 * - `auto-compact-long-history`：§2 的观察是「未触发」，Rust 侧断言在同一测试文件的
 *   `long_history_turn_does_not_auto_compact`；15 轮 × 100k 字符的回放不在套件里跑。
 * - `context-usage-settings`：T6 快照/读面对照在 `crates/agent-server/tests/snapshot.rs`。
 * - `fresh-install-ledger`：T7 账本对照在 `crates/agent-server/tests/lifecycle.rs`。
 */
const REGISTERED_SKIPS = [
  "compact-while-turn-running",
  "auto-compact-long-history",
  "context-usage-settings",
  "fresh-install-ledger",
] as const;

test("C2 夹具回放：覆盖清单与注册表逐字一致", () => {
  assert.deepEqual(Object.keys(C2_REPLAY_CASES).sort(), [...COVERED_CASES].sort());
});

test("C2 夹具回放：登记未覆盖清单是已知项", () => {
  const dir = fileURLToPath(new URL("./fixtures/c2/", import.meta.url));
  const fixtureCases = readdirSync(dir)
    .filter((entry) => entry.endsWith(".json"))
    .map((entry) => entry.replace(/\.json$/u, ""));
  for (const skip of REGISTERED_SKIPS) {
    assert.ok(fixtureCases.includes(skip), `登记的未覆盖项必须是真实存在的夹具：${skip}`);
  }
  // 除覆盖项与登记未覆盖项之外不允许有第三种状态（新夹具必须显式归位）。
  const accounted = new Set([
    ...COVERED_CASES.map((caseName) => caseName.split(":")[0] as string),
    ...REGISTERED_SKIPS,
  ]);
  for (const caseName of fixtureCases) {
    assert.ok(
      accounted.has(caseName),
      `夹具 ${caseName} 既不在覆盖清单也不在登记未覆盖清单`,
    );
  }
});

async function runCase(caseName: string): Promise<C2ReplayResult> {
  const driver = C2_REPLAY_CASES[caseName];
  assert.ok(driver, `没有 case driver：${caseName}`);
  const fixture = await loadC2Fixture(caseName.split(":")[0] as string);
  return driver(fixture);
}

for (const caseName of COVERED_CASES) {
  test(`Rust agent-server C2 回放：${caseName} 与夹具一致（C2 登记过滤）`, async () => {
    const result = await runCase(caseName);
    assert.deepEqual(
      result.diffs,
      [],
      `Rust agent-server 与 C2 夹具 ${caseName} 不一致：\n${formatDiffs(result.diffs)}\nnotes: ${JSON.stringify(result.notes)}`,
    );
  });
}
