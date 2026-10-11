# 自研插件署名（商店「开发者」= Mode）

## 背景

设置 → 插件商店详情「信息」区的「开发者」曾显示 `Z.ai`：官方市场 seed
（`apps/mode-cli/packages/bootstrap/src/app/official-plugin-definitions.ts` 的
author 常量）与随包插件 manifest（`.mode-plugin/plugin.json#author`）都沿用上游署名。
电脑控制等插件已改为本仓库自研分发（见 `computer-use-plugin-distribution.md`），
署名与实际维护方不符，用户据此要求改为自研信息。

## 产品规则

- 商店「开发者」与随包 manifest `author` 一律署名 `Mode`；不带站点 `url`——本仓库
  没有对外站点，`authorUrl` 会随 listing 投影给 UI，宁可缺省也不指向上游域名。
- 随包（`filesystem`/`sea`）官方插件的 listing 只来自 definitions 的 seed，唯一事实源是
  `MODE_AUTHOR` 一个常量；不在各条 definition 或 manifest 里散落硬编码。
- manifest `author` 是 listing 缺失时的兜底（详情页取
  `listing?.author ?? item.info?.author ?? describeMetadata?.author`），因此三个随包
  producer 包（browser-use / node-repl-host / mode-cua）的 `plugin.json` 与 seed 同步署名。
- 随包快照 `official-plugin-catalog.generated.ts` 保持上游官方目录逐条原样（不变式见
  `scripts/bundle-official-plugin-catalog.mjs` 头注释），**不改写其中的上游署名**：它只喂给
  本分支没有本地包的条目；随包 definitions 带 listing 的条目在 overview 兜底消费时
  **按名覆写为 definitions listing**（见下节数据流），不消费快照同名条目的 listing。
- 上游 `Z.ai` 只作为模型供应商文案存在于供应商设置区，与插件署名互不影响，不属本 spec 范围。

## 状态所有者与数据流

```
official-plugin-definitions.ts  MODE_AUTHOR（唯一事实源）
        │  listing.seed
        ▼
writeOfficialMarketplace（随包分片，每次 resolve 重建）
        │  按包名过滤 bundled catalog 快照同名条目
        ▼
merged marketplace.json → parseEntryStoreListing → listing.author
        │（注：官方平台服务下线（isOfficialServiceEnabled 恒 false）→ mode-plugins-official
        │  不进 DEFAULT 市场与 known 记录 → getModePluginsOverview 的 catalogs 读不到
        │  这份本地 canonical manifest；该路径不作为 overview 的实际输入）
        ▼
getModePluginsOverview 快照兜底（bundledCatalogPlugins）：
  · rawEntry.name ∈ OFFICIAL_PLUGIN_DEFINITIONS 且带 listing
      → listing 以 definitions 覆写（MODE_AUTHOR + zh 文案，icon 按快照口径剥离）
  · 其余条目（本分支没有本地包，含第三方）
      → 快照原样（上游署名保留）
        │  同时写 listingByPluginId（已装/内置 join 共用）
        ▼
plugins/overview availablePlugins → UI PluginStoreDetailView.InfoSection（开发者行）

.mode-plugin/plugin.json#author → plugins/describe → item.info.author /
                                  describeMetadata.author（listing 缺失时兜底）
```

## 验收场景

1. 电脑控制详情页「开发者」= `Mode`，信息区不再出现 `Z.ai`。
2. browser-use / node-repl-host / mode-cua 三个 `plugin.json` 的 `author.name` = `Mode`。
3. `packages/services/test/official-plugin-attribution.test.ts` 通过：definitions 只以
   `MODE_AUTHOR` 声明署名且值为 `Mode`，三个 manifest 文本不含 `Z.ai`。
4. `pnpm typecheck`、`pnpm lint` 通过。
5. `packages/services/test/cua-overview-attribution.test.ts` 通过（隔离 fixture）：
   overview 的 computer-use 条目 `listing.author = "Mode"`、无 `authorUrl`、
   `displayName_i18n.zh-CN = 电脑控制`；快照第三方条目（如 cloudbase-skills）署名保持原样。
   背景：f7d1405 只覆盖了 seed 侧，overview 快照兜底未按名覆写——实机 known 仅有
   claude 市场记录，详情「开发者」仍回流快照 Z.ai（本机复现后先 RED 后 GREEN）。
