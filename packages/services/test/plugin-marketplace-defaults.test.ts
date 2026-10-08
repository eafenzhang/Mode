import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";
import {
  BUILTIN_DEFAULT_MARKETPLACE_IDS,
  CLAUDE_PLUGIN_MARKETPLACE_ID,
  CLAUDE_PLUGIN_MARKETPLACE_SOURCE,
  CODEX_PLUGIN_MARKETPLACE_ID,
  CODEX_PLUGIN_MARKETPLACE_SOURCE,
  ZCODE_OFFICIAL_PLUGIN_MARKETPLACE_ID,
  DEFAULT_PLUGIN_MARKETPLACES,
  RETIRED_DEFAULT_MARKETPLACES,
  isBuiltinDefaultMarketplaceId,
  isCodexPluginMarketplaceId,
  isPublicStoreMarketplaceId,
  listingRequiresPaidPlan,
  resolveDefaultPluginMarketplaces,
} from "@zcode/shared";

// 需求演进：Codex 聚合目录与 Claude 官方目录都不再作为默认源；公开分段的候选条目
// 直接取 zcode-plugins-official 官方市场的清单，个人分段给用户放自己登记的源（Claude 等）。
// 这里钉住默认集合、退役清理清单与「预置源不可移除」的判据。

test("Codex / Claude 默认源已退役：都不再默认预置；只有 Codex 做退役清理", () => {
  const defaultIds = DEFAULT_PLUGIN_MARKETPLACES.map((item) => item.id);
  assert.ok(!defaultIds.includes(CODEX_PLUGIN_MARKETPLACE_ID), "Codex 源不应再默认预置");
  assert.ok(!defaultIds.includes(CLAUDE_PLUGIN_MARKETPLACE_ID), "Claude 源不应再默认预置");

  const retiredCodex = RETIRED_DEFAULT_MARKETPLACES.find(
    (entry) => entry.id === CODEX_PLUGIN_MARKETPLACE_ID,
  );
  assert.ok(retiredCodex, "Codex 源应登记为退役");
  assert.equal(retiredCodex.source, CODEX_PLUGIN_MARKETPLACE_SOURCE);
  assert.equal(CODEX_PLUGIN_MARKETPLACE_SOURCE, "hashgraph-online/awesome-codex-plugins");

  // Claude 官方目录不登记退役清理：清理只看 id + source，分不清是当年自动种下的还是
  // 用户自己刚登记的，一旦列入就会把个人分段要用的 Claude 源每次 overview 都删掉。
  assert.ok(
    !RETIRED_DEFAULT_MARKETPLACES.some(
      (entry) => entry.id === CLAUDE_PLUGIN_MARKETPLACE_ID,
    ),
    "Claude 源不得进入退役清理清单",
  );
  assert.equal(CLAUDE_PLUGIN_MARKETPLACE_SOURCE, "anthropics/claude-plugins-official");

  assert.ok(RETIRED_DEFAULT_MARKETPLACES.some((entry) => entry.id === "xiu86-codex-plugins"));
  const resolvedIds = resolveDefaultPluginMarketplaces().map((item) => item.id);
  assert.ok(!resolvedIds.includes(CODEX_PLUGIN_MARKETPLACE_ID));
});

test("预置源判据：官方目录是唯一预置源，且属于「公开」分段", () => {
  assert.equal(isBuiltinDefaultMarketplaceId(ZCODE_OFFICIAL_PLUGIN_MARKETPLACE_ID), true);
  assert.equal(isPublicStoreMarketplaceId(ZCODE_OFFICIAL_PLUGIN_MARKETPLACE_ID), true);
  assert.equal(isPublicStoreMarketplaceId(CODEX_PLUGIN_MARKETPLACE_ID), false);
  assert.deepEqual([...BUILTIN_DEFAULT_MARKETPLACE_IDS], [ZCODE_OFFICIAL_PLUGIN_MARKETPLACE_ID]);
  assert.equal(isCodexPluginMarketplaceId(CODEX_PLUGIN_MARKETPLACE_ID), true);
});

// 公开分段与官方目录保持一致，两件事缺一不可：
//   1) 商店不做内容裁剪 —— 公开条目直接取 zcode-plugins-official 市场的候选清单；
//   2) 随包官方清单与官方逐条一致 —— 官方目录里有哪些插件，随包快照就有哪些，
//      曾经按「本仓库有没有该插件目录」挑条目，结果官方有的 android-emulator /
//      ios-simulator 在商店里根本不出现，看着就像「和官方不一致」。
test("公开分段只认官方市场，随包官方清单不裁剪", async () => {
  const listView = await readFile(
    new URL(
      "../../../packages/ui/src/settings/PluginStoreListView.tsx",
      import.meta.url,
    ),
    "utf8",
  );
  assert.ok(
    listView.includes("items.filter((item) => isPublicStoreMarketplaceId(item.marketplace))"),
    "公开分段必须直接取官方市场的候选条目，不能再做商店侧裁剪",
  );

  const snapshot = await readFile(
    new URL(
      "../../../apps/zcode-cli/packages/bootstrap/src/app/official-plugin-catalog.generated.ts",
      import.meta.url,
    ),
    "utf8",
  );
  const entryCount = (snapshot.match(/"description":/gu) ?? []).length;
  assert.ok(entryCount >= 40, `随包官方清单条目过少（${entryCount}），像是又被裁剪过`);
  // 官方只有随包/兜底才能装的第一方插件：清单里必须在，否则商店与官方对不上。
  assert.ok(snapshot.includes('"name": "android-emulator"'), "官方清单里的第一方插件不得被挑掉");
  assert.ok(snapshot.includes('"name": "ios-simulator"'), "官方清单里的第一方插件不得被挑掉");
  assert.ok(snapshot.includes('"source": "url"'), "官方清单的远端条目必须原样保留");
  assert.ok(snapshot.includes("description_i18n"), "官方清单自带的中文描述必须保留");
  assert.ok(!snapshot.includes('"cachePath":'), "随包清单不得写入 per-install 的 cachePath");
  assert.ok(!snapshot.includes('"icon":'), "随包清单不得携带 CDN 图标地址（图标按插件名随包匹配）");
});

// 官方清单的 icon 指向 CDN、用户自加目录（Claude 等）的图标也不该被请求：
// 图标一律按「插件名」匹配随包资源，个人分段与公开分段共用同一份原版图标。
test("插件图标按插件名随包兜底", async () => {
  const iconSource = await readFile(
    new URL("../../../packages/ui/src/lib/pluginIconSource.ts", import.meta.url),
    "utf8",
  );
  assert.ok(iconSource.includes("ZCODE_OFFICIAL_PLUGIN_ICON_BY_NAME"), "图标必须按插件名匹配随包资源");
  // 曾经踩过：兜底查的是按完整 id（name@marketplace）索引的总表，裸插件名永远查不到，
  // 官方源靠 id 精确命中看着正常、用户自加目录一律退回字母占位。锁住兜底查的表。
  assert.ok(
    iconSource.includes(
      "const byName = BUNDLED_PLUGIN_ICON_BY_NAME[at > 0 ? pluginId.slice(0, at) : pluginId];",
    ),
    "同名兜底必须查按插件名索引的表，不能拿按 id 索引的总表当兜底",
  );
  assert.ok(
    iconSource.includes("...ZCODE_OFFICIAL_PLUGIN_ICON_BY_NAME,"),
    "按插件名的表必须由生成的随包图标映射构成",
  );
});
// 源码契约：加载器必须认识 Codex 市场的约定位置，否则默认源永远拉不出目录。
test("CLI 加载器搜索 .agents/plugins/marketplace.json（Codex 约定）", async () => {
  const source = await readFile(
    new URL(
      "../../../apps/zcode-cli/packages/adapters/src/plugins/marketplace.ts",
      import.meta.url,
    ),
    "utf8",
  ).catch(() => null);
  if (!source) {
    return; // 单包视角（无 monorepo 全量检出）时跳过
  }
  assert.ok(
    source.includes('join(".agents", "plugins", "marketplace.json")'),
    "缺少 Codex 市场 manifest 路径常量",
  );
  const findBlock = source.slice(
    source.indexOf("function findMarketplaceManifestPath"),
    source.indexOf("function findMarketplaceManifestPath") + 600,
  );
  assert.ok(
    findBlock.includes("CODEX_MARKETPLACE_FILE"),
    "manifest 搜索候选必须包含 Codex 约定位置",
  );
  // 已有 ZCode/Claude 布局的解析顺序不能被改变：Codex 候选放在最后。
  assert.ok(
    findBlock.indexOf("MARKETPLACE_FILE") < findBlock.indexOf("CODEX_MARKETPLACE_FILE"),
    "Codex 候选必须排在 ZCode/Claude 布局之后",
  );
});

// 源码契约：大 catalog（聚合目录）不能靠整仓归档，插件源码要能按需取。
test("加载器支持 raw manifest 快路径与 Codex local 源按需检出", async () => {
  const source = await readFile(
    new URL(
      "../../../apps/zcode-cli/packages/adapters/src/plugins/marketplace.ts",
      import.meta.url,
    ),
    "utf8",
  ).catch(() => null);
  if (!source) {
    return;
  }
  assert.ok(
    source.includes("requestGitHubRawMarketplaceManifest") &&
      source.includes("raw.githubusercontent.com"),
    "github 市场必须先尝试 raw manifest（聚合目录整仓归档几十 MB，慢链路必然超时）",
  );
  assert.ok(
    source.includes('if (sourceKind === "local")') &&
      source.includes("fetchMarketplacePluginFromRepo"),
    "Codex 约定的 { source: local, path } 条目必须支持，并在目录未落盘时按需稀疏检出",
  );
  assert.ok(
    source.includes("pruneRetiredDefaultMarketplaces") &&
      source.includes("RETIRED_DEFAULT_MARKETPLACES"),
    "退役默认源必须在种子阶段清理",
  );
});

// 源码契约：插件图标要用目录条目/插件 manifest 里的原始图标，而不是自己画一个。
test("目录条目自带 icon 解析成仓库直链；只有真的没有原始图标才用字母兜底", async () => {
  const source = await readFile(
    new URL(
      "../../../apps/zcode-cli/packages/adapters/src/plugins/marketplace-plugin-icons.ts",
      import.meta.url,
    ),
    "utf8",
  ).catch(() => null);
  if (!source) {
    return;
  }
  assert.ok(
    source.includes("resolveCatalogIconRepoPath") &&
      source.includes("resolveRawGitHubUrl(input.repo, input.ref, repoRelative)"),
    "目录条目的相对 icon 必须解析成市场仓库的 raw 直链（社区目录 78% 的条目自带 icon）",
  );
  assert.ok(
    source.includes("interface") && source.includes("composerIcon"),
    "没有 icon 的条目要读插件 manifest 的 interface.composerIcon/logo 兜底",
  );
  assert.ok(
    source.includes("requestMarketplaceAssetExists"),
    "候选图标路径要探测存在性，避免 404 直链覆盖默认图标",
  );
  const uiSource = await readFile(
    new URL("../../../packages/ui/src/components/PluginIcon.tsx", import.meta.url),
    "utf8",
  ).catch(() => null);
  if (uiSource) {
    assert.ok(
      uiSource.includes("Blocks"),
      "UI 侧缺少原始图标时用默认图标兜底，不再生成替代图标",
    );
    assert.ok(
      !uiSource.includes("resolvePluginMonogram"),
      "不应再生成字母图标：没有图标的插件统一用默认图标",
    );
  }
});

// 公开分段必须真的能把官方插件显示出来：随包官方清单要并进候选列表，
// 且 host 侧的插件总览不得再把官方市场的条目整段滤掉（滤掉时公开分段恒为空，
// 只剩「官方市场已下线」空状态，和「公开是官方插件」的诉求正相反）。
test("官方市场投影：随包清单并入候选，总览不再过滤官方条目", async () => {
  const bootstrap = await readFile(
    new URL("../../../apps/zcode-cli/packages/bootstrap/src/plugins.ts", import.meta.url),
    "utf8",
  );
  assert.ok(
    bootstrap.includes("BUNDLED_OFFICIAL_PLUGIN_CATALOG") &&
      bootstrap.includes("[...bundledCatalogPlugins, ...availablePlugins]"),
    "随包官方清单必须并入候选插件列表，否则公开分段没有官方条目",
  );

  const service = await readFile(
    new URL("../../../packages/services/src/plugins/pluginManagementService.ts", import.meta.url),
    "utf8",
  );
  assert.ok(
    service.includes("officialMarketplaceEnabled: true"),
    "官方市场投影必须照常下发（目录随包、浏览零网络）",
  );
  assert.ok(
    !service.includes("plugin.marketplace !== ZCODE_OFFICIAL_PLUGIN_MARKETPLACE_ID"),
    "总览不得再过滤官方插件，否则公开分段又变成空",
  );
});

// 需要编程套餐（付费套餐）才好用的插件不进商店：Mode 里官方平台已整体下线，套餐无法开通，
// 这类插件装上也只能报错。过滤发生在「候选层」（bootstrap 总览）与「展示层」（UI 商店列表），
// 两处必须用同一个谓词，判据是目录条目自带的 requiresPaidPlan 标记。
test("需编程套餐的插件不进候选与商店列表", async () => {
  assert.equal(listingRequiresPaidPlan({ requiresPaidPlan: true }), true);
  assert.equal(listingRequiresPaidPlan({ requiresPaidPlan: false }), false);
  assert.equal(listingRequiresPaidPlan({}), false);
  assert.equal(listingRequiresPaidPlan(undefined), false, "缺标记时不得误伤（fail open 会让过滤失效，必须判 false 而不报错）");

  const bootstrap = await readFile(
    new URL("../../../apps/zcode-cli/packages/bootstrap/src/plugins.ts", import.meta.url),
    "utf8",
  );
  assert.ok(
    bootstrap.includes("listingRequiresPaidPlan(entry.listing)") &&
      bootstrap.includes("if (!listingRequiresPaidPlan(listing))"),
    "总览的两条候选路径（市场目录 + 随包清单）都必须过滤需套餐条目",
  );

  const listing = await readFile(
    new URL("../../../packages/ui/src/settings/pluginStoreListing.ts", import.meta.url),
    "utf8",
  );
  assert.ok(
    listing.includes("if (listingRequiresPaidPlan(summary.listing)) continue;"),
    "商店列表必须用同一谓词再过滤一次（老版本 agent 的旧总览也不能漏出来）",
  );

  const snapshot = await readFile(
    new URL(
      "../../../apps/zcode-cli/packages/bootstrap/src/app/official-plugin-catalog.generated.ts",
      import.meta.url,
    ),
    "utf8",
  );
  assert.ok(
    snapshot.includes('"requiresPaidPlan": true'),
    "随包官方清单必须原样保留 requiresPaidPlan 标记：清掉标记等于把过滤放空",
  );
});

// 官方目录里有一批随包内置的第一方插件（computer-use、documents、pdf 等）是打进官方客户端的，
// 本仓库只随包了 browser-use / node-repl-host：条目照官方展示，但不能给一个点了才报
// 「Bundled plugin cache directory missing」的安装入口。
test("未随包的第一方插件标为不可安装，而不是让用户点了报错", async () => {
  const adapter = await readFile(
    new URL("../../../apps/zcode-cli/packages/adapters/src/plugins/marketplace.ts", import.meta.url),
    "utf8",
  );
  assert.ok(
    adapter.includes("export function hasBundledPluginPackage("),
    "适配器要提供「随包安装包是否存在」的判据（filesystem/sea 来源按 cachePath 与约定缓存目录两处查）",
  );
  assert.ok(
    adapter.includes("ships inside the official client and is not bundled in this fork"),
    "缓存缺失时的报错要说清原因，别只丢一句 directory missing",
  );

  const bootstrap = await readFile(
    new URL("../../../apps/zcode-cli/packages/bootstrap/src/plugins.ts", import.meta.url),
    "utf8",
  );
  const callSites = bootstrap.split("hasBundledPluginPackage({").length - 1;
  assert.ok(callSites >= 2, `候选插件的两条路径（市场目录 + 随包清单）都要打标，实际 ${callSites} 处`);

  const card = await readFile(
    new URL("../../../packages/ui/src/settings/PluginStoreCard.tsx", import.meta.url),
    "utf8",
  );
  assert.ok(
    card.includes('data-testid="plugin-store-bundled-unavailable"') &&
      card.includes("if (item.bundledUnavailable)"),
    "商店卡片遇到这类条目要换成不可点的说明，而不是安装按钮",
  );
  const protocol = await readFile(
    new URL("../../../packages/shared/src/zcode-protocol/index.ts", import.meta.url),
    "utf8",
  );
  assert.ok(protocol.includes("bundledUnavailable: z.boolean().optional()"), "协议要带上这个标记");
});
