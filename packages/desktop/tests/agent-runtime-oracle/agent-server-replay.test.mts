// C1 Task 10：Rust `agent-server` 三层回放用例（`pnpm test:agent-runtime-oracle` 的成员）。
//
// 期望值来自 oracle 录制的行为夹具（`fixtures.mts` 的同一批 case），比较器是
// `compare.mts` + `normalize.mts` 的**同一份**实现；唯一附加的回放口径是
// 「C1 登记缺省过滤」（`replay-agent-server.mts` 的 `C1_ABSENT_EVENT_TYPES`，
// 两侧同删三个 C1 未产出的事件类型），它是过滤器不是第二张规则表。
// 未登记的差异会原样报红——那是本用例的产出，不是要绕开的东西。
//
// 前置条件：`cargo build -p agent-server`（缺失时 harness 自动构建一次）。
import { test } from "node:test";
import assert from "node:assert/strict";

import { formatDiffs } from "./compare.mjs";
import { loadFixtures } from "./fixtures.mjs";
import { isThreeLayerFixture, replayFixture } from "./replay-agent-server.mjs";

// `fixtures/` 根下还住着非三层夹具（session-stream.json 投影快照等，oracle 套件
// 对它们的基线已是红，Task 10 不修）；这里显式识别并跳过，留痕不静默。
const all = (await loadFixtures()) as unknown[];
const skipped = all.filter((fixture) => !isThreeLayerFixture(fixture)).length;
if (skipped > 0) {
  test(`Rust agent-server 夹具回放：跳过 ${skipped} 个非三层夹具（投影快照不属本轮回放）`, () => {
    assert.ok(skipped > 0);
  });
}

for (const fixture of all.filter(isThreeLayerFixture)) {
  test(`Rust agent-server 夹具回放：${fixture.case} 三层一致（C1 登记缺省过滤）`, async () => {
    const result = await replayFixture(fixture);
    assert.deepEqual(
      result.diffs,
      [],
      `Rust agent-server 与 oracle 夹具 ${fixture.case} 三层不一致：\n${formatDiffs(result.diffs)}`,
    );
  });
}
