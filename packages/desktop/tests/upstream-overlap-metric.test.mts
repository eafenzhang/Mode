/**
 * M4/M5 上游重合度指标的纯函数用例（spec：`docs/specs/upstream-overlap-metric.md`）。
 *
 * 真实 git 读取在脚本 main 里；这里只钉口径：品牌归一、行数、三档判定、聚合分母。
 * 指标工具不允许输出上游正文，因此测试也只用合成文本。
 */
import assert from "node:assert/strict";
import test from "node:test";

import {
  classifyPair,
  countLines,
  isDeferred,
  isExempt,
  normalizeBrand,
  parseLsTree,
  summarize,
  topLevelCounts,
} from "../../../scripts/upstream-overlap-check.ts";

test("品牌归一覆盖四种写法与 CRLF，不改动无关文本", () => {
  assert.equal(normalizeBrand("zcode"), "mode");
  assert.equal(normalizeBrand("ZCode"), "mode");
  assert.equal(normalizeBrand("Z-CODE"), "mode");
  assert.equal(normalizeBrand("Z_CODE"), "mode");
  assert.equal(normalizeBrand("ZCodium"), "mode");
  assert.equal(normalizeBrand("ZCODE_LITE_TOKEN"), "mode_LITE_TOKEN");
  assert.equal(normalizeBrand("a\r\nb"), "a\nb");
  assert.equal(normalizeBrand("const value = 1;\n"), "const value = 1;\n");
});

test("行数：空文本 0，末尾换行不产生空行", () => {
  assert.equal(countLines(""), 0);
  assert.equal(countLines("a"), 1);
  assert.equal(countLines("a\n"), 1);
  assert.equal(countLines("a\nb\n"), 2);
  assert.equal(countLines("a\n\n"), 2);
});

test("三档判定：同 blob → identical；归一相同 → normalized；其余 distinct", () => {
  assert.equal(
    classifyPair({ sameSha: true, oursText: "anything", upstreamText: "other" }),
    "identical",
  );
  assert.equal(
    classifyPair({
      sameSha: false,
      oursText: "mode config\n",
      upstreamText: "ZCode config\n",
    }),
    "normalized",
  );
  assert.equal(
    classifyPair({ sameSha: false, oursText: "added\n", upstreamText: "original\n" }),
    "distinct",
  );
  assert.equal(
    classifyPair({ sameSha: false, oursText: null, upstreamText: "text" }),
    "distinct",
  );
});

test("豁免前缀（vendored 与许可证原文）不参与 headline", () => {
  assert.equal(isExempt("third-party/copied-components.json"), true);
  assert.equal(isExempt(".agents/skills/foo/SKILL.md"), true);
  assert.equal(isExempt("scripts/license-texts/CC-BY-4.0.txt"), true);
  assert.equal(isExempt("packages/ui/src/app.tsx"), false);
  assert.equal(isExempt("third-partyish/file.ts"), false);
});

test("M4 范围豁免只覆盖 sidecar/传输层前缀", () => {
  for (const path of [
    "packages/services/src/node.ts",
    "packages/shared/src/git.ts",
    "packages/rpc/src/protocol.ts",
    "packages/server/src/http.ts",
    "packages/provider/src/config.ts",
  ]) {
    assert.equal(isDeferred(path), true, path);
  }
  assert.equal(isDeferred("packages/ui/src/App.tsx"), false);
  assert.equal(isDeferred("packages/desktop/src/host-standalone/index.ts"), false);
  assert.equal(isDeferred("packages/services-extra/x.ts"), false);
});

test("parseLsTree 只收 blob，跳过 tree 与 submodule", () => {
  const output = Buffer.from(
    [
      "100644 blob aaa111\tREADME.md",
      "100755 blob bbb222\tscripts/run.sh",
      "040000 tree ccc333\tsrc",
      "160000 commit ddd444\tvendor/sub",
      "100644 blob eee555\tsrc/index.ts",
      "",
    ].join("\0"),
  );
  const entries = parseLsTree(output);
  assert.deepEqual([...entries.keys()], ["README.md", "scripts/run.sh", "src/index.ts"]);
  assert.deepEqual(entries.get("scripts/run.sh"), { mode: "100755", sha: "bbb222" });
});

test("聚合：分母是同路径文本总行数，分子只含 identical + normalized", () => {
  const summary = summarize(
    [
      { path: "a.ts", tier: "identical", lines: 10 },
      { path: "b.ts", tier: "normalized", lines: 5 },
      { path: "c.ts", tier: "distinct", lines: 15 },
      { path: "bin.dat", tier: "distinct", lines: 0 },
    ],
    { oursOnly: 2, upstreamOnly: 3 },
  );

  assert.equal(summary.common, 4);
  assert.deepEqual(summary.identical, { files: 1, lines: 10 });
  assert.deepEqual(summary.normalized, { files: 1, lines: 5 });
  assert.deepEqual(summary.distinct, { files: 2, lines: 15 });
  assert.equal(summary.commonLines, 30);
  assert.equal(summary.overlapLines, 15);
  assert.equal(summary.overlapRatio, 0.5);
  assert.equal(summary.oursOnly, 2);
  assert.equal(summary.upstreamOnly, 3);
});

test("聚合：没有同路径文本时比率为 0，不产生 NaN", () => {
  const summary = summarize([], { oursOnly: 1, upstreamOnly: 0 });
  assert.equal(summary.commonLines, 0);
  assert.equal(summary.overlapRatio, 0);
});

test("顶层目录分布按档计数", () => {
  const counts = topLevelCounts([
    { path: "packages/a.ts", tier: "identical" },
    { path: "packages/b.ts", tier: "distinct" },
    { path: "crates/c.rs", tier: "normalized" },
    { path: "README.md", tier: "distinct" },
  ]);
  assert.deepEqual(counts.get("packages"), { identical: 1, normalized: 0, distinct: 1 });
  assert.deepEqual(counts.get("crates"), { identical: 0, normalized: 1, distinct: 0 });
  assert.deepEqual(counts.get("(root)"), { identical: 0, normalized: 0, distinct: 1 });
});
