# Spec：`model-selection` 频道（Rust View 装配）

状态：2026-11-22 设计（实现待接；renderer 高频使用，必须先保证语义等价）。

## 背景

`IModelSelectionService` 只有三个成员：`getView(input?)`、可选
`saveConfiguredDefault(selection)`、`onDidChange` 事件。Node 实现是
`createModelSelectionService(facade, ensureReady, configuredDefaultSource)`，其中
`facade` 为 `ModelSelectionFacade`。生产装配（`providerRuntime`）调用
`createNodeModelSelectionFacade(registryService)` **不传 classifier**，即实际只用
`ordinary` 分类；account-plan/account-offpeak 与 legacy reasoning 归一化在生产路径
未启用。本切片按该生产口径实现，避免引入未使用分支。

## 1. 数据源

| 事实 | 来源 |
| --- | --- |
| Built-in / Personal provider、model、顺序 | `provider_config_source::read_snapshot(bundled, personal, None)` + `ProviderConfigResolver` |
| configuredDefault（`config.defaultModelSelection`） | `PersonalConfigRepository::read().default_model_selection` |
| 变更通知 | `PersonalConfigRepository::on_did_change`（1s 轮询，revision 去重） |
| 写回 | `PersonalConfigRepository::update`（跨进程锁 + 原子写） |

`ProviderConfigResolver.resolve` 的 `registry_providers` 是唯一候选集合；
`visibility=hidden` 的 provider 从 View 中剔除（effective 解析同样按 hidden 拒绝）。

## 2. getView

入参：`ModelSelectionViewInput | undefined`；`{ selection: ModelSelection | null }`。
无入参时不输出 effective 字段；有入参时输出 `effectiveSelection`（可为 null）与可选
`selectionIssue`。

输出（与 TS `ModelSelectionView` 同构）：

```
{ revision: number,
  providers: [{ providerId, providerName, templateId, config, models: [{ modelId, config }] }],
  preferredSelection?: ModelSelection,
  effectiveSelection?: ModelSelection | null,
  selectionIssue?: "selection-missing" | "provider-not-found" | "model-not-found"
                  | "reasoning-level-missing" | "reasoning-level-not-supported" }
```

- `config` 用 `ProviderConfig::to_json()` / `ModelConfig::to_json()`（已有 TS 对拍）。
- **preferred**：configuredDefault 可选中（provider 非 hidden、model 存在、档位合法）
  时用它；否则按 registry 顺序取首个非 hidden provider 的首个 model，并补最高档
  （`reasoningLevel = values.last()`）；都没有则省略字段。
- **effective（ordinary）**：provider 缺失/hidden → `provider-not-found`；
  model 缺失 → `model-not-found`；档位缺失 → `reasoning-level-missing`；
  档位不在 values → `reasoning-level-not-supported`。有效时保留完整 options，
  否则只保留 `{providerId, modelId}`。
- **revision**：Host 进程内单调计数器（从 1 起）。配置快照 revision 变化或
  repository 通知到达时 +1，保证 UI 的 stale 判断只增不减。

## 3. saveConfiguredDefault

写回 `config.defaultModelSelection`：`repository.update` 保留现有 providers/models/
order，仅替换该字段（`ProviderConfigLayerUpdate`）。未装配写能力时命令显式报错
（对应 TS 可选方法的 Method-not-found 语义）。

## 4. onDidChange

- 事件名 `onDidChange`；注册监听时不立即补发（TS Event 同义）。
- 首次注册时挂 `repository.on_did_change`（Weak 引用防泄漏）：变更到达 → revision+1
  → 重新计算 View 并广播给全部监听者；读取失败只记日志、不广播坏快照。
- Built-in 文件是随包静态资产，不在轮询范围（TS Registry 也只在配置源变更时重建）。

## 5. 装配

- `host_channels::model_selection::ModelSelectionChannel`：入参 bundled 路径 +
  personal 路径；自建 `PersonalConfigRepository`（1s 轮询，首次 `read()` 启动）。
- Desktop inprocess：由壳注入 bundled 路径（复用
  `agent_runtime::resolve_provider_snapshot_paths`），personal =
  `<data_root>/v2/provider_config.json`；无 agent control 依赖。
- 无 bundled 路径（server-mode / 未注入）时频道不可构造，回落 Node；进入
  inprocess 默认集后 desktop renderer 全部走 Rust。
- Node 服务保留为回退（`MODE_HOST_INPROCESS_CHANNELS=none`）。

## 6. 已知差异与边界

1. 只实现生产实际使用的 ordinary 分类；account-plan/account-offpeak 与 legacy
   reasoning 归一化显式不做（生产 facade 未传 classifier/resolver）。
2. Built-in 变更不订阅（随包静态资产）。
3. `getView` 每次读文件（builtin+personal）；personal 层同时被 repository 轮询，
   两份读取可能相差一个轮询周期，revision 由 snapshot + 通知共同推进，不承诺强一致。

## 7. 验收

- `cargo test -p host-channels --lib model_selection`：preferred 选择（configuredDefault /
  fallback / 空）、effective 五类 issue、hidden 过滤、revision 单调。
- 真机：模型选择器打开、切换默认模型、`/model` 同语义写回、外部改
  `provider_config.json` 后选择器自动刷新。
