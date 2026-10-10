# 电脑控制启用权威（设置开关收口）

状态：已拍板（2026-10-10）。关联：[computer-use-windows-runtime.md](./computer-use-windows-runtime.md)（runtime 与 Helper 本体，本文只管「何时启用」）。

## 背景与问题

电脑控制（Computer Use，自研 `@mode/cua` runtime + Helper）的启用链路此前有三处错位：

1. 设置页「电脑控制」分区被放进 `HIDDEN_SETTINGS_SECTIONS`（`packages/ui/src/lib/settingsNavigation.ts`），带总开关的页面在 UI 中不可达。
2. 设置总开关写的是插件启用态 `computer-use@mode-plugins-official`，但 services 侧权威判定 `isCuaEnabledForContext` 是 `env || 插件启用` 的旁路：打包层 env `isModeCuaInternalFeatureEnabled` 默认 ON，导致**关闭开关后 win32 spawn 仍会预热自研 Helper、仍注入 broker 凭据**，开关管不住自研链路。
3. 注释与文案按「官方插件」口径描述，而官方实现本就是占位包，实际执行的全部是自研 runtime。

## 产品规则与状态所有者

- **唯一状态**：插件配置 `enabledPlugins["computer-use@mode-plugins-official"]`（user / project 层 `config.json`、`mode.json`、`.mode/config.json` 合并投影）。
- **唯一写路径**：`pluginManagementStore.setEnabled` → `pluginManagementService`。设置页「电脑控制」分区与插件页插件行是同一状态的两个 UI 读写面，不允许出现第二个开关字段或派生缓存。
- **默认关闭**：`enabledPlugins[id] ?? defaultEnabled(false)`，不改 `OFFICIAL_PLUGIN_DEFINITIONS` 的 `defaultEnabled` 名单（bootstrap 单测机械对照两处）。
- **用户可见语义**：开关即「启用/关闭本机自研电脑控制及其 MCP 与技能」；关闭后已有对话需重启 Mode 生效（沿用既有 toast `settings.computerUse.disabledToast`）。

## 门控矩阵

```text
打包层 env  isModeCuaInternalFeatureEnabled   → kill-switch：只能关、不能开
    │  （MODE_CUA_PRODUCT_HELPER=0/false/off → seed 抑制、helper 不建）
    ▼ AND
用户权威门  resolveCuaWorkspaceEnablement     → env && 插件启用态
    │  消费方：helper 创建 admission / spawn broker 注入 /
    │          动态 resolver isPluginEnabled / 权限服务可用性
    ▼ demand 边界
resolver③  isOfficialCuaPluginEnabledForWorkspace → 拉起自研 host 前再判一次（同源）
```

- env 为 OFF 时即使配置残留 `true` 也全链路压住（stale config 不得越过 kill-switch）。
- env 为 ON（默认）时，开关是唯一权威；`MODE_CUA_DEV_MODE=1` 不再隐式启用，开发者需在设置里显式开启一次（写盘后持久化）。
- 插件 seed 分发、恢复入口仍由 env 门控（存在性问题，不归开关管）。

## 事件顺序与生效边界

```text
用户拨开关 → pluginManagementStore.setEnabled → 配置写盘（幂等，重复切换收敛到最终值）
   → 新 spawn / 新 resolveMcpServers 按权威门生效（懒启动，无常驻轮询）
   → 已有对话的工具集不热切换：提示重启 Mode（既有 toast）
macOS：权限服务可用性随权威门变化 → 权限行在「启用后」才出现（先启用、后授权）
```

## UI 规则

- 分区位置：设置 → 基础设置，紧跟「浏览器」（`settingsPageConfig.ts` 既有排序）。
- 桌面本机 workspace（win/mac）显示总开关；远端 workspace 与 Linux 显示「当前环境暂不支持」卡片；Web 不出现该分区（`createSettingsPageConfig` 平台门 + `SETTINGS_SECTIONS` 显式排除）。
- 输入框「电脑操作」按钮仍由 `computerUseComposerEntryHidden` 单独控制（默认隐藏），其开关随分区放开后可从设置页触达。

## 迁移说明

- `HIDDEN_SETTINGS_SECTIONS` 移除 `"computerUse"`；`resolveSettingsSection` / 输入框按钮跳转 intent 随之恢复直达。
- 常量名 `MODE_CUA_OFFICIAL_PLUGIN_ID` 属实现细节，不改名。
- 不改插件市场、seed、默认启用名单。

## 验收场景

- [ ] 桌面（win/mac）设置导航出现「电脑控制」，页内含总开关与「在输入框显示电脑操作按钮」。
- [ ] Web 设置导航不出现该分区；远端 / Linux 进入分区只见 unsupported 卡片。
- [ ] 关闭开关：win32 spawn 不预热 Helper、不注入 broker 凭据；动态 resolver 不启用；macOS 权限查询报未启用。
- [ ] 开启开关：helper 创建 admission 放行，resolver 在 demand 边界拉起自研 Helper 并注入 socket/authority。
- [ ] `MODE_CUA_PRODUCT_HELPER=0` + 配置残留 `true` → 全链路关闭。
- [ ] `MODE_CUA_DEV_MODE=1` + 未启用 → 关闭（不再绕过）。
- [ ] 单测：`resolveCuaWorkspaceEnablement` 覆盖 kill-switch / 启用 / 默认关 / dev 不绕过 / suppressed 五例；设置配置断言桌面含 `computerUse`、Web 不含、`isSettingsSectionEnabled("computerUse")` 为真。

## 非目标

- 不做 `MODE_CUA_PRODUCT_HELPER=0` 环境下开关「死开关」的 UI 防护（内部 kill-switch 场景，维持现状）。
- 不改用户可见文案措辞（「电脑控制」即产品名），仅改代码注释与 spec 的语义归属。
- 不动 macOS TCC 授权流与 Windows Helper runtime 实现（见关联 spec）。
