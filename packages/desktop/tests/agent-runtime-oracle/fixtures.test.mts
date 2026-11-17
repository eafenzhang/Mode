import { test } from "node:test";
import assert from "node:assert/strict";

import { compareAll, formatDiffs } from "./compare.mjs";
import { FIXTURE_CASES, loadFixtures, observeCase } from "./fixtures.mjs";
import { normalizeValue, RECORDED_NORMALIZE_CONTEXT, stableStringify } from "./normalize.mjs";

const RAW_UUID = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/i;

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

// 录制守卫：`expect` 里不该剩下原始 uuid。
// 它把「白名单漏了一个键」这类问题从「下次回放随机变红」提前成「录完立刻可见」。
// （不是万能的：`msg_<随机>_<uuid>` 的前缀部分没有 uuid 形状，那种漏网要靠回放用例抓——
//  实测抓到过一次 `anchorId`，已加入 normalize.mts 的 id 键集合。）
for (const fixture of await loadFixtures()) {
  test(`录制守卫：${fixture.case} 的 expect 不含未规范化的 uuid`, () => {
    assert.doesNotMatch(
      stableStringify(fixture.expect),
      RAW_UUID,
      `${fixture.case} 的 expect 里有原始 uuid：normalize.mts 的白名单漏了键，夹具不可复现`,
    );
  });
}

// 每条夹具都回放一遍：录下来的期望值必须能被同一个 oracle 再次复现，否则夹具不可用。
//
// 期望侧也要过**当前**的 §3 白名单（C1 Task 11）：夹具是录制时的规范化结果，录制之后新增
// 的规则（stack 删除 / JSON 文本对象键序 / 命令指纹）必须同样折叠到录制值上。对既有规则
// 这一步是幂等的，所以既有绿不会变红；录制后新增规则带来的既有红（dist 栈漂移）会变绿。
for (const fixture of await loadFixtures()) {
  test(`夹具回放：${fixture.case} 三层一致`, async () => {
    const observed = await observeCase(fixture);
    const expected = {
      ...fixture.expect,
      events: fixture.expect.events.map((event) => ({
        type: event.type,
        payload: normalizeValue(event.payload, RECORDED_NORMALIZE_CONTEXT),
      })),
    };
    const diffs = compareAll(observed, expected);
    assert.deepEqual(diffs, [], `夹具 ${fixture.case} 三层不一致：\n${formatDiffs(diffs)}`);
  });
}
