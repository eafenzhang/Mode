# 插件商店可用性状态（可用 / 未随包 / 源已下线）

## 背景

插件商店「公开」分段直接展示官方目录的全部条目（随包快照，离线可浏览），但其中有两类条目在本分支**确定拿不到安装包**：

1. **未随包（`bundledUnavailable`）**：source 为 `filesystem`/`sea` 的第一方插件由官方客户端随包分发，本仓库不含其安装包，磁盘缓存里也没有。
2. **源已下线（`sourceUnavailable`）**：source 是指向 `zcode.z.ai` / `cdn-zcode.z.ai` 的 zip 下载地址。官方平台已整体断连（见 `packages/shared/src/officialPlatformPolicy.ts`，HTTP 出口对所有请求断言），describe 要下载 zip、安装也要下载 zip，两者**必然**报「官方平台服务已下线，marketplace 功能不再可用」。

这两类如果照常给「安装」按钮、详情页照常发起 `plugins/describe`，用户看到的都是「点了必失败 / 组件清单加载失败（重试也失败）」，把确定性不可用呈现成了可重试的临时故障。

## 产品规则

| 条目状态 | 商店卡片 | 详情页 · 组件区 | describe |
| --- | --- | --- | --- |
| 可安装（有本地包，或 source 可达） | 安装按钮 | `plugins/describe` 按需枚举；失败走「加载失败 + 重试」 | 发起 |
| 未随包 `bundledUnavailable` | 禁用 pill「未随包提供」 | 固定说明（`bundledUnavailableHint`），无重试 | 跳过 |
| 源已下线 `sourceUnavailable` | 禁用 pill「源已下线」 | 固定说明（`sourceUnavailableHint`），无重试 | 跳过 |

- 已安装条目不受这两个标记影响：详情页用运行时权威枚举（`ModePluginInfo.components`）。
- 只有**真正的临时失败**（网络抖动、来源损坏）才呈现「组件清单加载失败，仅展示可得信息。」并提供重试。

## 状态所有者与数据流

判定只在 bootstrap 做一次，UI 不重复推断来源：

```
bootstrap（唯一判定点）
  ├─ hasBundledPluginPackage(...)        → bundledUnavailable   （filesystem/sea 且无缓存包）
  └─ isOfficialOfflinePluginSource(...)  → sourceUnavailable    （source URL 命中已下线官方域名）
        │
        ▼  plugins/overview
mode-protocol 投影 toAvailablePluginSummary 原样透出两个标记（漏投影 = 商店复活必败安装入口）
        │
        ▼  strict zod: modeAvailablePluginSummarySchema（packages/shared/src/mode-protocol）
services pluginManagementService → ui pluginManagementStore
        │
        ▼  buildStoreItems → StorePluginItem
UI 渲染：安装按钮禁用 pill / 详情静态说明 / 跳过 describe
```

- 协议 schema 是 `.strict()`：CLI 侧新字段必须同步进 schema，否则 overview 整包校验失败。
- 判定谓词 `isOfficialOfflinePluginSource` 放在 `packages/shared/src/plugin-marketplaces.ts`（与 `listingRequiresPaidPlan` 同处），复用 `officialPlatformPolicy.isOfficialPlatformUrl`，域名表变更时自动跟随。

## 接口

- `ModeAvailablePluginSummary.sourceUnavailable?: boolean`（协议，strict schema）
- `isOfficialOfflinePluginSource(source: unknown): boolean`（shared）
- UI 纯函数 `resolvePluginDetailComponentsState(...)`（`packages/ui/src/settings/pluginDetailComponentsState.ts`）：
  `bundled` | `sourceUnavailable` | `loading` | `error` | `ready`

## 验收场景

1. 打开「未随包提供」插件详情：显示固定说明，不出现「组件清单加载失败」，网络面板无 `plugins/describe`。
2. 打开 source 为官方 CDN zip 的插件详情：同上，固定说明为「源已下线」。
3. 两类卡片上的安装 pill 禁用且带 hover 提示；不可点击。
4. source 可达的候选插件（如 Claude 目录 github 源）：describe 正常，组件分区照常；真实失败时仍是「加载失败 + 重试」。
5. 已安装插件详情不受标记影响，仍显示权威组件枚举。
6. 两个标记经协议 `.strict()` schema 往返不丢（bootstrap 投影测试钉住）。
