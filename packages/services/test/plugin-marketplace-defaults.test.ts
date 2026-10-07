import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";
import {
  BUILTIN_DEFAULT_MARKETPLACE_IDS,
  CODEX_PLUGIN_MARKETPLACE_ID,
  CODEX_PLUGIN_MARKETPLACE_SOURCE,
  DEFAULT_PLUGIN_MARKETPLACES,
  RETIRED_DEFAULT_MARKETPLACES,
  isBuiltinDefaultMarketplaceId,
  isCodexPluginMarketplaceId,
  isPublicStoreMarketplaceId,
  resolveDefaultPluginMarketplaces,
} from "@zcode/shared";

// 需求：插件市场默认带上 Codex 插件源（.agents/plugins/marketplace.json 约定），
// 并且这个默认源要能在刷新后自动补种（不会被页面刷新/重启清掉），插件自带图标缺失时
// 由 UI 用市场品牌图标兜底。这里钉住默认集合与「预置源不可移除」的判据。

test("默认市场集合包含 Codex 插件源，且不受官方市场开关影响", () => {
  const codex = DEFAULT_PLUGIN_MARKETPLACES.find((item) => item.id === CODEX_PLUGIN_MARKETPLACE_ID);
  assert.ok(codex, "缺少 Codex 默认市场");
  assert.equal(codex.source, CODEX_PLUGIN_MARKETPLACE_SOURCE);
  // id 必须与目标市场 manifest 声明的 name 一致，否则刷新时会多出一条挂空记录。
  assert.equal(codex.id, codex.name);

  // 官方源受 isOfficialServiceEnabled 过滤；Codex 源是普通来源，任何开关下都在默认集合里。
  const resolvedIds = resolveDefaultPluginMarketplaces().map((item) => item.id);
  assert.ok(resolvedIds.includes(CODEX_PLUGIN_MARKETPLACE_ID), "Codex 源应始终进入默认集合");
});

test("预置源判据：Codex 源不可移除，且不算「公开」分段", () => {
  assert.equal(isBuiltinDefaultMarketplaceId(CODEX_PLUGIN_MARKETPLACE_ID), true);
  assert.equal(isCodexPluginMarketplaceId(CODEX_PLUGIN_MARKETPLACE_ID), true);
  // 公开分段仍只有官方市场：Codex 源不改变商店分段语义。
  assert.equal(isPublicStoreMarketplaceId(CODEX_PLUGIN_MARKETPLACE_ID), false);
  assert.ok(
    (BUILTIN_DEFAULT_MARKETPLACE_IDS as readonly string[]).includes(CODEX_PLUGIN_MARKETPLACE_ID),
  );
});

// 默认源指向的是社区聚合目录（codex 生态全部插件），不是单个作者仓库：
// 早期默认源只有一个插件，用户反馈"添加了 codex 源却看不到所有插件"。
test("默认 Codex 源是聚合目录，且旧默认源进入退役清单", () => {
  assert.equal(CODEX_PLUGIN_MARKETPLACE_SOURCE, "hashgraph-online/awesome-codex-plugins");
  const retired = RETIRED_DEFAULT_MARKETPLACES.find((entry) => entry.id === "xiu86-codex-plugins");
  assert.ok(retired, "旧默认源应登记为退役，种子阶段精确清理");
  assert.equal(retired.source, "xiu86/codex-plugins");
  assert.ok(
    !DEFAULT_PLUGIN_MARKETPLACES.some((entry) => entry.id === retired.id),
    "退役源不应再出现在默认集合里",
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
