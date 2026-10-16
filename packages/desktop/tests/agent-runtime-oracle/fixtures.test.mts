import { test } from "node:test";
import assert from "node:assert/strict";

import { compareAll, formatDiffs } from "./compare.mjs";
import { FIXTURE_CASES, loadFixtures, observeCase } from "./fixtures.mjs";

test("夹具目录覆盖 Step 5 要求的三条用例", async () => {
  const fixtures = await loadFixtures();
  const names = fixtures.map((fixture) => fixture.case).sort();
  const expectedNames = FIXTURE_CASES.map((fixtureCase) => fixtureCase.case).sort();
  assert.deepEqual(
    names,
    expectedNames,
    "夹具文件与 record.mts 的用例表不一致（缺夹具或多了手写的夹具）",
  );
  for (const fixture of fixtures) {
    assert.ok(fixture.expect, `${fixture.case} 缺 expect：夹具必须由 oracle 录制`);
    assert.ok(
      Array.isArray(fixture.expect.toolCalls) && fixture.expect.events.length > 0,
      `${fixture.case} 的 expect 是空的：录制没有真正跑起来`,
    );
  }
});

// 每条夹具都回放一遍：录下来的期望值必须能被同一个 oracle 再次复现，否则夹具不可用。
for (const fixture of await loadFixtures()) {
  test(`夹具回放：${fixture.case} 三层一致`, async () => {
    const observed = await observeCase(fixture);
    const diffs = compareAll(observed, fixture.expect);
    assert.deepEqual(diffs, [], `夹具 ${fixture.case} 三层不一致：\n${formatDiffs(diffs)}`);
  });
}
