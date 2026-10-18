# M3 生态域：插件市场迁移现状与决策（2026-10-18）

> 路线图 M3 的「插件市场迁移」验收是：读旧 `installed_plugins.json` / marketplace 缓存
> 原样可用；市场卡片 E2E 复用 `plugin-store-availability` spec 的场景。
> 本文件记录**现有实现与测试证据**，并按与 LAN/IM bot 相同的口径给出决策。

## 结论先行

**插件市场由 Host sidecar 复用 `packages/services` 与 `packages/ui` 的现有实现，
M3 不新增 `crates/marketplace`。** 数据层与卡片状态的兼容性已由 32 条服务测试覆盖；
剩余是**运行中应用的 UI E2E**（spec 6 个验收场景）与 **Node sidecar 打包**（CDN 资产、
真实 `assetUrl`），后者与插件宿主 Task 6 的发布形态绑定，不是市场本身的代码缺口。

## 归属证据

- `packages/services/src/plugins/pluginsService.ts` 在 Host 进程内装配
  （`packages/desktop/src/host/index.ts` 经 `@mode/services` 取服务），Tauri 的 Node sidecar
  复用同一份实现（M3 生态计划 DR-M3-1）。
- `packages/ui/src/settings/PluginStorePage.tsx` / `PluginStoreDetailView.tsx` 走
  `usePlugins` → `pluginManagementStore` → 服务面，没有 Electron 专有依赖；M3 挂载的
  存量 renderer 直接可用。
- 官方目录快照、市场 seed 与深链保留在数据层；「公开」浏览分段已按用户决策删除
  （`plugin-store-availability.md`「公开分段移除」一节），市场卡片默认是个人分段。

## 兼容性实现（只读，不改写）

| 兼容点 | 实现 | 证据 |
| --- | --- | --- |
| 旧 `installed_plugins.json` | `installedPluginRoots.ts` 读 `plugins[]`，要求绝对 `installPath`，坏记录跳过；`id`/`marketplace` 读时归一到 canonical 名 | `plugin-marketplace-id-compat.test.ts` |
| 旧官方市场 id | 读时归一，旧名仍归入对应分段 | `plugin-marketplace-id-compat.test.ts`、`plugin-marketplace-defaults.test.ts` |
| 官方插件缓存根 | 新目录优先，缺失时回落旧目录 | 同上（「官方插件缓存根」用例） |
| 未随包 / 源已下线 | bootstrap 唯一判定点产出 `bundledUnavailable` / `sourceUnavailable`，经 strict zod 投影 | `official-plugin-attribution.test.ts`、`plugin-store-availability.md` |
| 详情页组件区状态 | 纯函数 `resolvePluginDetailComponentsState`：`unbundled` / `sourceUnavailable` / `loading` / `error` / `ready` | `plugin-detail-components-state.test.ts` |
| 描述翻译 / 目录默认源 | 现有实现与测试 | `plugin-description-translation.test.ts` |

`installed_plugins.json`、`.mode-plugin` 清单与市场缓存**只读**：M3 不迁移、不改写，
写路径仍只有 Host services 一条（与 `2026-10-18-m3-plugin-host.md` 的 Task 0 结论一致）。

## 测试证据（2026-10-18 实跑）

```bash
cd packages/services
pnpm exec tsx --test test/plugin-*.test.ts test/official-plugin-attribution.test.ts \
  test/cua-plugin-distribution.test.ts
```

结果：**32 tests / 32 pass / 0 fail**，覆盖市场 id 兼容、默认源、缓存根回落、
不可安装标记、详情状态、官方归因与 CUA 分发。

另外补了卡片投影的 node 侧用例 `packages/ui/test/pluginStoreListing.test.ts`
（`pnpm --filter @mode/ui test` 的一部分）：**5 tests / 5 pass**，钉住
`buildStoreItems` 的不可安装标记透出、付费套餐条目过滤、缺包插件不生成安装入口、
孤立安装判定与恢复态。它不启窗口，因此不能替代 UI E2E，但能防止「商店复活必败安装入口」
这类纯投影回归。

## DR-PM-1：M3 不新增 `crates/marketplace`（2026-10-18）

| 选项 | 含义 | 代价 |
| --- | --- | --- |
| **(a) sidecar 复用（采纳）** | 市场数据与安装流程继续由 `packages/services` 提供；M3 只验证兼容与卡片状态 | 与插件宿主、LAN、IM bot 同一架构；无重复状态 |
| (b) 用 Rust 重写市场目录与安装 | `crates/marketplace` 解析 `.mode-plugin`、维护目录与安装事务 | 与 sidecar 内既有实现重复；安装事务涉及远程下载/校验，M3 无收益 |

(b) 延后到 M4/M5 把插件运行时移出 sidecar 时再评估；届时以
`2026-10-18-m3-plugin-host.md` 的 manifest/AssetCache 结论为起点。

## 仍未完成项

**必须由人 / 需要发布形态**：

1. **市场卡片 UI E2E**：`plugin-store-availability` 的 6 个验收场景需要运行中的应用
   （打开两类不可安装详情、确认无 `plugins/describe`、安装 pill 禁用、已安装详情不受影响）；
   目前只有服务层与纯函数测试，**没有真实窗口的自动化证据**。
2. **Node sidecar 打包**：插件宿主 bundle 上 CDN、真实 `assetUrl`、Tauri 壳按发布形态
   spawn（`2026-10-18-m3-plugin-host.md` Task 6 的发布侧）。
3. **断网降级**：插件组件缺失时的提示文案与核心功能不受影响，需要在打包形态下验收。

**可自动执行的后续**：

- 市场卡片的投影层已补（`packages/ui/test/pluginStoreListing.test.ts`）；
- 继续 Web/手机远控与 TUI 域盘点。
