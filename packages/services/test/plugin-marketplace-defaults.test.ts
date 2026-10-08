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
  resolveDefaultPluginMarketplaces,
} from "@zcode/shared";

// 需求演进：Codex 聚合目录与 Claude 官方目录都不再作为默认源（改为退役清单精确清理），
// 公开分段固定为随包内置的 ZCode 官方插件目录（zcode-plugins-official）。
// 这里钉住默认集合、退役清单与「预置源不可移除」的判据。

test("Codex / Claude 默认源已退役：不在默认集合，且登记为精确清理", () => {
  const defaultIds = DEFAULT_PLUGIN_MARKETPLACES.map((item) => item.id);
  assert.ok(!defaultIds.includes(CODEX_PLUGIN_MARKETPLACE_ID), "Codex 源不应再默认预置");
  assert.ok(!defaultIds.includes(CLAUDE_PLUGIN_MARKETPLACE_ID), "Claude 源不应再默认预置");

  const retiredCodex = RETIRED_DEFAULT_MARKETPLACES.find(
    (entry) => entry.id === CODEX_PLUGIN_MARKETPLACE_ID,
  );
  assert.ok(retiredCodex, "Codex 源应登记为退役");
  assert.equal(retiredCodex.source, CODEX_PLUGIN_MARKETPLACE_SOURCE);
  assert.equal(CODEX_PLUGIN_MARKETPLACE_SOURCE, "hashgraph-online/awesome-codex-plugins");

  const retiredClaude = RETIRED_DEFAULT_MARKETPLACES.find(
    (entry) => entry.id === CLAUDE_PLUGIN_MARKETPLACE_ID,
  );
  assert.ok(retiredClaude, "Claude 源应登记为退役");
  assert.equal(retiredClaude.source, CLAUDE_PLUGIN_MARKETPLACE_SOURCE);

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

// 公开分段的浏览面来自随包内置的官方目录快照（离线可用，不连 CDN）。
test("随包官方目录快照存在，id 与市场常量一致且条目充足", async () => {
  const source = await readFile(
    new URL(
      "../../../apps/zcode-cli/packages/bootstrap/src/app/official-plugin-catalog.generated.ts",
      import.meta.url,
    ),
    "utf8",
  );
  assert.ok(source.includes('"name": "zcode-plugins-official"'), "快照 name 必须等于市场 id");
  const entryCount = (source.match(/"name": "/gu) ?? []).length;
  assert.ok(entryCount >= 20, `随包目录条目过少：${entryCount}`);
  assert.ok(source.includes('"source": "url"'), "应保留 url 来源条目");
  assert.ok(!source.includes("\"cachePath\":"), "快照不得写入 per-install 的 cachePath");
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
