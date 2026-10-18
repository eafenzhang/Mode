/**
 * M4 T2 台账覆盖门禁的纯函数用例（spec/计划：`docs/plans/2026-10-18-m4-third-party-ledger.md`）。
 *
 * 真实 `cargo metadata` 在 CLI 里读取；这里用合成图钉住口径：发布目标集合、可达闭包、
 * name@version 精确匹配、版本漂移与无 license 标识的 fail-closed。
 */
import assert from "node:assert/strict";
import test from "node:test";

import {
  auditLedger,
  collectReachable,
  defaultTargets,
  indexLedger,
  type CargoMetadata,
  type LedgerComponent,
} from "../../../scripts/rust-ledger-gap.ts";

const metadata: CargoMetadata = {
  packages: [
    { id: "ws:shell", name: "mode-tauri", version: "0.1.0", license: null, source: null },
    { id: "ws:test", name: "tauri-test-manifest", version: "0.1.0", license: null, source: null },
    { id: "reg:alpha1", name: "alpha", version: "1.0.0", license: "MIT", source: "registry" },
    { id: "reg:alpha2", name: "alpha", version: "2.0.0", license: "MIT", source: "registry" },
    { id: "reg:beta", name: "beta", version: "1.0.0", license: "Apache-2.0", source: "registry" },
    { id: "reg:gamma", name: "gamma", version: "0.1.0", license: null, source: "registry" },
  ],
  resolve: {
    nodes: [
      { id: "ws:shell", deps: [{ pkg: "reg:alpha1" }, { pkg: "reg:alpha2" }] },
      { id: "ws:test", deps: [] },
      { id: "reg:alpha1", deps: [{ pkg: "reg:beta" }] },
      { id: "reg:alpha2", deps: [{ pkg: "reg:gamma" }] },
      { id: "reg:beta", deps: [] },
      { id: "reg:gamma", deps: [] },
    ],
  },
};

const ledger: LedgerComponent[] = [{ id: "alpha", version: "1.0.0", license: "MIT" }];

test("台账索引按 name@version 精确匹配，并记录同名的已知版本", () => {
  const index = indexLedger(ledger);
  assert.equal(index.exact.has("alpha@1.0.0"), true);
  assert.equal(index.exact.has("alpha@2.0.0"), false);
  assert.deepEqual([...(index.versionsByName.get("alpha") ?? [])], ["1.0.0"]);
  assert.equal(index.versionsByName.has("beta"), false);
});

test("默认发布目标排除测试/校验专用成员", () => {
  assert.deepEqual(defaultTargets(metadata), ["mode-tauri"]);
});

test("可达闭包包含传递依赖，未知目标直接报错", () => {
  const reachable = collectReachable(metadata, ["mode-tauri"]);
  assert.deepEqual(
    [...reachable].sort(),
    ["reg:alpha1", "reg:alpha2", "reg:beta", "reg:gamma", "ws:shell"],
  );
  assert.throws(() => collectReachable(metadata, ["missing-root"]), /workspace 目标不存在/);
});

test("审计区分缺项与版本漂移，并把无 license 的外部 crate 单列", () => {
  const audit = auditLedger(metadata, ledger, ["mode-tauri"]);
  const target = audit.targets[0];
  assert.equal(target.target, "mode-tauri");
  assert.equal(target.external, 4);
  assert.equal(target.covered, 1);
  assert.deepEqual(target.missing, [
    "alpha@2.0.0 [MIT]",
    "beta@1.0.0 [Apache-2.0]",
    "gamma@0.1.0 [NO-LICENSE]",
  ]);
  assert.deepEqual(target.versionDrift, ["alpha@2.0.0 [MIT]"]);
  assert.deepEqual(audit.unknownLicense, ["gamma@0.1.0"]);
  assert.deepEqual(audit.union.missing, target.missing);
  assert.equal(audit.ledgerComponents, 1);
});

test("并集按 name@version 去重，不重复计算共享依赖", () => {
  const shared: CargoMetadata = {
    packages: [
      { id: "ws:a", name: "crate-a", version: "0.1.0", license: null, source: null },
      { id: "ws:b", name: "crate-b", version: "0.1.0", license: null, source: null },
      { id: "reg:x", name: "shared", version: "1.0.0", license: "MIT", source: "registry" },
    ],
    resolve: {
      nodes: [
        { id: "ws:a", deps: [{ pkg: "reg:x" }] },
        { id: "ws:b", deps: [{ pkg: "reg:x" }] },
        { id: "reg:x", deps: [] },
      ],
    },
  };
  const audit = auditLedger(shared, ledger, ["crate-a", "crate-b"]);
  assert.equal(audit.targets.length, 2);
  assert.equal(audit.targets[0].external, 1);
  assert.equal(audit.targets[1].external, 1);
  assert.deepEqual(audit.union.missing, ["shared@1.0.0 [MIT]"]);
});
