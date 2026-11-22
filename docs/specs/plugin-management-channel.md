# Spec：`plugin-management` 频道（Rust 薄别名）

状态：2026-11-22 新建（收口 Node-only 频道）。

## 背景

TS `IPluginManagementService` 是设置页「插件管理」的薄服务：18 个方法全部直接
转发 `IModeAgentService` 的同名方法（`packages/services/src/plugins/pluginManagementService.ts`），
唯一加工是 `getPluginsOverview` 强制 `officialMarketplaceEnabled: true`
（官方市场目录随包内置、离线可浏览）。

Rust `mode-agent` 频道已实现这些 plugins 命令（list/overview/describe/
setEnabled/install/uninstall/update/configure/validate/referenceCatalog/
restoreBuiltin/resetConfig/marketplace 增删改/cancelPluginOperation/
suggested 引用解析）与 `onDynamicPluginOperationProgress` 订阅。因此本频道不需要
再实现一遍协议，只做**同名转发 + overview 投影**。

## 1. 行为

- `call`：方法名与参数原样转发给 `mode-agent` 频道；`getPluginsOverview` 的
  JSON 结果补 `officialMarketplaceEnabled: true`（已是 true 则保持不变）。
- `listen`：`onDynamicPluginOperationProgress` 原样委托 `mode-agent` 的 listen。
- 未知方法与 mode-agent 一致返回 not-yet-migrated 错误（由内层频道给出）。

## 2. 装配

`host_channels::plugin_management::PluginManagementChannel` 持有一个独立的
`ModeAgentChannel`（与 `mode-agent` 频道同参数：agent control + 管理面 workspace）。
进入 inprocess 默认集与 server-mode 批量注册；无 agent control 时不可构造。
Node 服务保留为回退（`MODE_HOST_INPROCESS_CHANNELS=none`）。

## 3. 已知差异

1. 插件事件订阅由别名频道自己的 `ModeAgentChannel` 实例承载（与 mode-agent 频道
   各持一份监听表）；插件进度按 operationId 过滤，互不影响。
2. `officialMarketplaceEnabled` 恒为 true：与 TS 当前产品口径一致（官方平台服务
   下线不影响随包内置插件目录）。

## 4. 验收

- `cargo test -p host-channels --lib plugin_management`：转发、overview 投影、
  listen 委托、无 agent control 不可构造。
- 真机：设置页插件安装/启停/取消、进度条、公开市场分段、建议引用解析。
