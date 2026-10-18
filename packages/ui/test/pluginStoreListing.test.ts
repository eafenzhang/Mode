/**
 * 插件商店卡片投影（M3 插件市场迁移的静态验证面）。
 *
 * `plugin-store-availability.md` 的卡片规则由 `buildStoreItems` 统一决定；这里在 node 下
 * 直接跑投影（不启窗口），钉住四条会直接影响「安装按钮是否出现」的语义：
 *
 * 1. `bundledUnavailable` / `sourceUnavailable` 原样透出，且不会因目录存在而被当成已安装；
 * 2. 需要付费套餐的目录条目不出现在商店（与 bootstrap 用同一谓词）；
 * 3. 运行时发现但缺包的插件不生成安装入口；
 * 4. 孤立安装只在「来源可用性已知 + source=cache + 市场不存在」时为 true。
 */
import assert from "node:assert/strict";
import test from "node:test";

const { buildStoreItems } = await import("../src/settings/pluginStoreListing.js");

const MARKETPLACE = "mode-plugins-official";

function available(overrides: Record<string, unknown>): Record<string, unknown> {
  return { id: "plugin@market", name: "plugin", marketplace: MARKETPLACE, installed: false, ...overrides };
}

function runtimeInfo(overrides: Record<string, unknown>): Record<string, unknown> {
  return {
    id: "runtime@market",
    name: "runtime",
    enabled: false,
    source: "cache",
    marketplace: MARKETPLACE,
    skillRootCount: 0,
    commandRootCount: 0,
    mcpServerNames: [],
    rootPath: "/tmp/plugin",
    ...overrides,
  };
}

function build(input: {
  availablePlugins?: Record<string, unknown>[];
  installedPlugins?: Record<string, unknown>[];
  plugins?: Record<string, unknown>[];
  restorableBuiltins?: Record<string, unknown>[];
  marketplaceAvailabilityKnown?: boolean;
}) {
  return buildStoreItems({
    marketplaces: [],
    marketplaceAvailabilityKnown: input.marketplaceAvailabilityKnown ?? false,
    availablePlugins: input.availablePlugins ?? [],
    installedPlugins: input.installedPlugins ?? [],
    plugins: input.plugins ?? [],
    restorableBuiltins: input.restorableBuiltins ?? [],
  } as never);
}

test("未随包与源已下线标记原样透出，且不产生已安装态", () => {
  const items = build({
    availablePlugins: [
      available({ id: "bundled@market", name: "bundled", bundledUnavailable: true }),
      available({ id: "offline@market", name: "offline", sourceUnavailable: true }),
    ],
  });

  const bundled = items.find((item) => item.id === "bundled@market");
  const offline = items.find((item) => item.id === "offline@market");
  assert.deepEqual(
    { bundledUnavailable: bundled?.bundledUnavailable, installed: bundled?.installed },
    { bundledUnavailable: true, installed: false },
  );
  assert.deepEqual(
    { sourceUnavailable: offline?.sourceUnavailable, installed: offline?.installed },
    { sourceUnavailable: true, installed: false },
  );
});

test("需要付费套餐的目录条目不进商店", () => {
  const items = build({
    availablePlugins: [
      available({ id: "paid@market", listing: { requiresPaidPlan: true } }),
      available({ id: "free@market" }),
    ],
  });

  assert.deepEqual(
    items.map((item) => item.id),
    ["free@market"],
  );
});

test("运行时发现但缺包的插件不生成安装入口", () => {
  const items = build({
    plugins: [runtimeInfo({ id: "missing@market", packageStatus: "missing" })],
  });

  assert.equal(items.length, 0);
});

test("孤立安装：只有来源可用性已知、source=cache 且市场不存在时为 true", () => {
  const info = runtimeInfo({ id: "orphan@market", marketplace: "removed-marketplace" });

  const known = build({ plugins: [info], marketplaceAvailabilityKnown: true });
  assert.equal(known[0]?.orphaned, true);

  const unknown = build({ plugins: [info], marketplaceAvailabilityKnown: false });
  assert.equal(unknown[0]?.orphaned, false);

  const official = build({
    plugins: [runtimeInfo({ id: "official@market", source: "official" })],
    marketplaceAvailabilityKnown: true,
  });
  assert.equal(official[0]?.orphaned, false);
});

test("已安装目录条目 join 运行时信息，恢复态不与实际安装态混淆", () => {
  const items = build({
    availablePlugins: [available({ id: "installed@market", installed: true })],
    installedPlugins: [
      {
        id: "installed@market",
        name: "installed",
        marketplace: MARKETPLACE,
        enabled: true,
        scope: "user",
      },
    ],
    plugins: [runtimeInfo({ id: "installed@market" })],
    restorableBuiltins: [available({ id: "restorable@market", name: "restorable" })],
  });

  const installed = items.find((item) => item.id === "installed@market");
  assert.equal(installed?.installed, true);
  assert.equal(installed?.restorable, false);
  assert.equal(installed?.info?.id, "installed@market");

  const restorable = items.find((item) => item.id === "restorable@market");
  assert.equal(restorable?.restorable, true);
  assert.equal(restorable?.installed, false);
});
