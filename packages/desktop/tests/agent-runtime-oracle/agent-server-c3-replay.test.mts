// C3 Task 5：Rust `agent-server` 的 C3 夹具回放用例（`pnpm test:agent-runtime-oracle` 的成员）。
//
// 期望值来自 C3 oracle 行为夹具（`fixtures/c3/*.json`，probe-c3-*.mts 机器录制），
// 比较器与 C1/C2 同一份（`normalize`/`session-lifecycle-fixtures`/`compare.mts`）；
// 回放口径（登记缺省过滤、登记折叠、可比子集）见 `replay-c3.mts` 文件头与
// `tauri-agent-runtime.md` §C3。未登记的差异原样报红——那是本用例的产出。
//
// 前置条件：`cargo build -p agent-server`（缺失时 harness 自动构建一次）。
import { readdirSync } from "node:fs";
import { test } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";

import { formatDiffs } from "./compare.mjs";
// driver 模块先于注册表取用（replay-c3 不反向静态依赖 driver，避免 import 环）。
import { registerC3GoalCases } from "./replay-c3-goal.mjs";
import { registerC3BackgroundCases } from "./replay-c3-background.mjs";
import { registerC3GoalConflictCases } from "./replay-c3-goal-conflict.mjs";
import { registerC3MixedCases } from "./replay-c3-mixed.mjs";
import { C3_REPLAY_CASES, loadC3Fixture, type C3ReplayResult } from "./replay-c3.mjs";

registerC3GoalCases(C3_REPLAY_CASES);
registerC3BackgroundCases(C3_REPLAY_CASES);
registerC3GoalConflictCases(C3_REPLAY_CASES);
registerC3MixedCases(C3_REPLAY_CASES);

/**
 * 已知 C3 覆盖清单：本套件逐条断言的 case（registry 注册面）。
 * 新增/减少都要显式更新本清单，不能静默吞掉。
 */
const COVERED_CASES = [
  "goal-show-set-complete",
  "goal-pause-resume-clear",
  "goal-replace-revision-conflict",
  "goal-continuation-loop",
  "retry-5xx-then-success",
  "retry-401-non-retryable",
  "retry-stream-abort",
  "retry-cancel-during-backoff",
  "background-idle",
  "background-busy-coalesced",
  "subagents-empty",
] as const;

/**
 * 登记未覆盖（`tauri-agent-runtime.md` §C3；新夹具必须显式归位）：
 * - `goal-cold-restart-paused/active/complete`：跨进程重启需要「杀掉再起」的窗口，
 *   stdio 驱动的 stop() 是协作退出而非 kill；冷恢复语义由 Rust 单测
 *   `crates/agent-server/tests/goal.rs` 的 `cold_restart_*` 断言（active→paused、
 *   target 账务保留、不自动续跑、显式 resume 完成 + T5 补的 `runtime_command_1`/
 *   `goalIteration:1` 序号断言）。
 * - `goal-user-stop` / `goal-pause-in-flight` / `goal-set-over-existing` /
 *   `goal-verifier-invalid-json`：交互/序号/fallback 语义由
 *   `crates/agent-server/tests/goal.rs` 对应用例断言（pause_in_flight、user stop、
 *   set-over-existing、invalid-JSON fallback）。
 * - `retry-backoff-sequence`：连续 3 次 503 的退避序列由 `crates/agent-core/tests/recovery.rs`
 *   的多失败用例断言（回放不重复长退避）。
 * - `subagents-foreground` / `subagents-background` / `subagents-pagination`：
 *   Rust runtime 无 spawn/child runtime（C4 面），无法端到端驱动 Agent 工具；
 *   读投影由 `crates/agent-server/tests/subagents.rs`（18 用例，直接种库）断言。
 * - `background-goal-interaction`：notification turn 在 verifier 前且计入目标由
 *   `crates/agent-server/tests/background.rs` 断言。
 * - `stream-recovery-c1-c2-shapes`：静态提取面（C1 夹具去重形状），无可执行语义；
 *   recovery 流形状由 `retry-stream-abort` 覆盖。
 */
const REGISTERED_SKIPS = [
  "goal-cold-restart-paused",
  "goal-cold-restart-active",
  "goal-cold-restart-complete",
  "goal-user-stop",
  "goal-pause-in-flight",
  "goal-set-over-existing",
  "goal-verifier-invalid-json",
  "retry-backoff-sequence",
  "subagents-foreground",
  "subagents-background",
  "subagents-pagination",
  "background-goal-interaction",
  "stream-recovery-c1-c2-shapes",
] as const;

test("C3 夹具回放：覆盖清单与注册表逐字一致", () => {
  assert.deepEqual(Object.keys(C3_REPLAY_CASES).sort(), [...COVERED_CASES].sort());
});

test("C3 夹具回放：登记未覆盖清单是已知项", () => {
  const dir = fileURLToPath(new URL("./fixtures/c3/", import.meta.url));
  const fixtureCases = readdirSync(dir)
    .filter((entry) => entry.endsWith(".json"))
    .map((entry) => entry.replace(/\.json$/u, ""));
  for (const skip of REGISTERED_SKIPS) {
    assert.ok(fixtureCases.includes(skip), `登记的未覆盖项必须是真实存在的夹具：${skip}`);
  }
  // 除覆盖项与登记未覆盖项之外不允许有第三种状态（新夹具必须显式归位）。
  for (const caseName of fixtureCases) {
    assert.ok(
      (COVERED_CASES as readonly string[]).includes(caseName) ||
        (REGISTERED_SKIPS as readonly string[]).includes(caseName),
      `夹具 ${caseName} 既不在覆盖清单也不在登记未覆盖清单`,
    );
  }
});

async function runCase(caseName: string): Promise<C3ReplayResult> {
  const driver = C3_REPLAY_CASES[caseName];
  assert.ok(driver, `没有 case driver：${caseName}`);
  const fixture = await loadC3Fixture(caseName);
  return driver(fixture);
}

for (const caseName of COVERED_CASES) {
  test(`Rust agent-server C3 回放：${caseName} 与夹具一致（C3 登记过滤）`, async () => {
    const result = await runCase(caseName);
    assert.deepEqual(
      result.diffs,
      [],
      `Rust agent-server 与 C3 夹具 ${caseName} 不一致：\n${formatDiffs(result.diffs)}\nnotes: ${JSON.stringify(result.notes)}`,
    );
  });
}
