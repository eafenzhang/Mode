import { test } from "node:test";
import assert from "node:assert/strict";

import { compareAll, formatDiffs } from "./compare.mjs";
import { FIXTURE_CASES, observeCase } from "./fixtures.mjs";

// 计划风险表要求的自检（spec §4.4）：同一 prompt 连跑两次，
// 工具调用序列与应用事件流除白名单字段外必须逐字相同——否则对拍用例会随机红。
// 用「同一次 turn 里两次工具调用」那条用例，顺带把调度顺序也钉住。
const REPEATABILITY_CASE =
  FIXTURE_CASES.find((fixtureCase) => fixtureCase.case === "read-twice-one-turn") ??
  FIXTURE_CASES[0];

function scriptedToolCallCount(fixtureCase: (typeof FIXTURE_CASES)[number]): number {
  return fixtureCase.script.reduce(
    (total, step) => total + ("toolCalls" in step ? step.toolCalls.length : 0),
    0,
  );
}

test("可重复性：同 prompt 连跑两次，三层逐字相同", async () => {
  const fixtureCase = REPEATABILITY_CASE;
  assert.ok(fixtureCase, "用例表是空的");

  const first = await observeCase(fixtureCase);
  const second = await observeCase(fixtureCase);

  const diffs = compareAll(second, first);
  assert.deepEqual(
    diffs,
    [],
    `同一个 prompt 两次运行结果不同（mock provider 的确定性被破坏）：\n${formatDiffs(diffs)}`,
  );
  // 「两次都空」也是一致，但那证明不了什么：钉住剧本里的工具调用真的跑出来了。
  assert.equal(
    first.toolCalls.length,
    scriptedToolCallCount(fixtureCase),
    "工具调用条数与剧本不符：这次运行没有真正走到工具执行",
  );
});
