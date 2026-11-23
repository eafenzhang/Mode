# Spec：`provider-settings` 频道（Rust，分阶段迁移）

状态：**2026-11-23 阶段 0–4 全部已落**（精确规则访问器 / View 投影 / 10 个写入方法 /
连通性测试 / 远端模型目录 / 频道 + onDidChange + inprocess 装配）；已进默认集，剩真机走查。

## 背景

`IProviderSettingsService`（`packages/services/src/model-provider/providerFacadeServices.ts`）
是设置页 Provider/Model 的唯一写面：View（分层配置 + issues + executable）与 14 个
mutation（create/save/delete/reorder provider/model、enable、draft、连通性测试、
远端目录）。Node 实现是 `ProviderSettingsFacade` + `SettingsMutationTarget`
（`packages/provider/src/facades.ts`，688 行）。

阶段 0（本 spec 覆盖）：`provider-config` 增加 TS `getExact` / `getExactRule`
等价访问器，供 View 投影复用，不新增第二套规则匹配。

## 1. 阶段 0：精确规则访问器（已完成）

`ModelConfigRules`：
- `get_exact(provider_id, model_id)`：按插入序叠加所有精确规则
  （`ModelRule::Provider` 与 `ModelRule::Manual`），后者字段覆盖前者（`ModelConfig::overlay`）。
- `get_exact_rule(provider_id, model_id)`：返回**最后一条**精确规则的类型
  （`ExactModelRuleKind::Provider | Manual`）与配置；Manual 决定
  `useRecommendedConfig=false`。
- 测试：叠加、最后一条类型、缺失返回 None。

## 2. 阶段 1：View 投影（2026-11-23 已落）

数据源：`provider-config-source::read_snapshot` + `ProviderConfigResolver` +
Personal 仓库 revision。输出与 `ProviderSettingsView` 同构：

- `providerTemplates`：Builtin 模板表（templateId/nameMap/config.toJSON）。
- `providerOrder`：`resolved_providers` 中 `config.group == "standard-personal"`
  且非 hidden 的顺序（TS 同义）。
- `providers`：`resolved_providers` 非 hidden；每项
  `{providerId, providerName, templateId, enabled, executable, templateConfig?,
  effectiveBuiltinConfig?, personalConfig?, effectiveConfig, issues, models[]}`；
  `executable = registry_providers` 包含该 id。
- models：`{kind, modelId, builtin, effectiveBuiltinConfig, personalExactConfig?,
  useRecommendedConfig?, effectiveConfig, enabled, executable, selectable, issues}`；
  `personalExactConfig` 用阶段 0 的 `get_exact`，`useRecommendedConfig` 用
  `get_exact_rule` 类型。
- `accountState`：本 fork 官方账号体系已下线，account overlay 状态为空；
  View 暂不输出该字段（与运行时一致，登记差异）。
- revision：Host 进程内单调计数（快照 revision / 仓库通知推进），与
  model-selection 同口径。
- 实现：`host-channels::provider_settings::build_provider_settings_view(snapshot, revision)`；
  `personalConfig` 取 Personal 规则的 `config`（ProviderConfigObject），待 fixture
  对拍确认与 TS 逐字段一致；`accountState` 暂不输出。

## 3. 阶段 2：写入面（地基已落，方法待做）

地基（2026-11-23）：`ProviderConfigMap.get_rule/set_rule/delete`、
`ModelConfigRules.delete_exact_for_provider`、`provider_resolver::resolve_owned_order`
公开，供 create/save/delete/reorder 复用。

已落（2026-11-23）：
- `create_personal_provider`：模板实例/空白 Provider；id 种子（模板 id 归一）
  + `-N` 去重、名称按 locale 取模板名并做大小写不敏感去重；模板实例不注入
  ApiKey access；group=standard-personal + personalModelIds/modelOrder 空默认；
  顺序按 `resolve_owned_order` 追加。
- `delete_personal_provider` / `reorder_personal_providers`（同上）。
- 用例 7 条（create 3 条 + delete/reorder + view）。

已落补充（2026-11-23）：

- `save_personal_provider_overlay`：membership CAS（providerId + personalRevision；
  TS 的外部 assertCurrent 不适用 Host，登记差异）、account provider 禁用/access
  守卫、builtin 组不可改写（保存时去掉 group）、personal-only 强制
  standard-personal、`normalizePersonalProviderMembership` +
  `withModelMembershipFrom`、`assertProviderLabelMutationIsUnique`（仅拦截本次
  变更新引入的重名）、metadata 覆盖 identity 字段。
- 用例 10 条（新增：builtin overlay 去 group/保留 key、account 守卫、未知
  provider/membership revision/重名、成员归一）。

已落补充（2026-11-23）：

- `ModelConfigRules::set_exact / delete_exact / rename_exact_model`（TS 同义：
  setExact 保留首条位置并去重、可切 manual；rename 保持位置）。
- `set_personal_model_enabled`：只改最新 enabled，写精确规则（沿用原类型），
  不清成员/顺序；builtin/personal 成员均可启停，缺失拒绝。
- `reorder_personal_models`：顺序只含 builtin+personal 成员；membership
  inheritedModelIds 优先。
- 用例：provider-config 17 条、host provider_settings 13 条。

已落补充 2（2026-11-23）：`add_personal_model`（enabled 强制 true、成员追加、
builtin/personal 冲突拒绝）、`rename_personal_model`（builtin 归属保护、成员与
精确规则改名、顺序映射）、`delete_personal_model`（builtin 保护、成员/规则清理）、
`save_personal_model_draft`（expectedRevision CAS、recommended/manual 类型沿用、
结构空 + recommended 删除精确规则）。阶段 2 的 10 个方法全部完成，15 条用例。

阶段 3 已落（2026-11-23）：

- `agent-process::test_model_connectivity`：legacy `provider/testModelConnectivity`
  请求（workspace + selection），结果必须是 `{success:true}`，失败以请求错误上抛；
  `mode-agent` 频道新增同名命令。
- `provider-model-listing::fetch_remote_model_catalog`：按 Provider API 类型构造
  请求，HTTP GET 分页（`after` 游标，最多 10 页/15s），按 id 去重聚合；404 附
  Base URL 含 /v1 的提示；纯读不写配置。本地两页分页用例。

待做：阶段 4（provider-settings 频道 + onDidChange + inprocess 装配）。

## 3. 阶段 2：写入面（已完成，2026-11-23）

10 个方法全部落在 `host-channels::provider_settings`（`pub fn` 纯函数 + `PersonalConfigRepository`
写入由调用方持锁）：`create_personal_provider` / `save_personal_provider_overlay` /
`delete_personal_provider` / `reorder_personal_providers` / `reorder_personal_models` /
`add_personal_model` / `rename_personal_model` / `delete_personal_model` /
`save_personal_model_draft` / `set_personal_model_enabled`；15 条用例。
仍待频道层补齐的部分见上（步骤 5「频道与装配」）。

## 4. 阶段 3：连通性 / 远端目录（已完成，2026-11-23）

- `testModelConnectivity`：`agent-process::test_model_connectivity` 发 legacy
  `provider/testModelConnectivity` 请求（workspace + selection），成功必须是
  `{success:true}`，失败以请求错误上抛；`host-channels::mode_agent` 已加同名命令。
- `listRemoteModels`：`crates/provider-model-listing::fetch_remote_model_catalog`——按 Provider
  API 类型构造请求，HTTP GET 分页（`after` 游标，最多 10 页 / 15s），按 id 去重聚合；
  404 附「Base URL 是否含 /v1」提示；纯读不写配置。本地两页分页用例。
  另修正 OrcaRouter `listModels` 对 `ProxyChannel` 数组参数的解析（`d47534748`）。

## 5. 阶段 4：频道与装配（2026-11-23 已落）

**频道** `host-channels::provider_settings_channel`（`PROVIDER_SETTINGS_CHANNEL = "provider-settings"`）：
15 个方法同名映射，位置参数按 `ProxyChannel` 约定（`arg` 是数组，对象在首位）。

| 方法 | 语义 | 备注 |
| --- | --- | --- |
| `getView` | 读快照 → `build_provider_settings_view` | revision = 进程内计数器，快照 revision 变更时 +1 |
| `refresh(reason)` | 重读快照后返回 View | Rust 每次 `getView` 都重读文件，`reason` 只进日志 |
| `createPersonalProvider(input?)` | 模板实例/空白 Provider | 返回 `{providerId, view}` |
| `resolveModelConfig(input)` | 合成 Rule 解析 + `validateComplete` | 无 `personalConfig` = `composeEffective(builtin, personal)`，inherited 与 effective **同源**；有 = inherited 只取 Builtin、effective 叠加草稿（`setExact(..., useRecommended=true)`），不落盘；问题路径带 `["providers", providerId, "models", modelId]` 前缀 |
| `savePersonalProviderOverlay(providerId, config, metadata?)` | 覆盖写 | 返回新 View |
| `deletePersonalProvider(providerId)` / `reorderPersonalProviders(ids)` | 删除 / 排序 | 返回新 View |
| `reorderPersonalModels(providerId, ids)` / `addPersonalModel(providerId, modelId, config, useRecommended?)` / `renamePersonalModel(providerId, from, to)` / `deletePersonalModel(providerId, modelId)` / `setPersonalModelEnabled(providerId, modelId, enabled)` | 模型写面 | 返回新 View |
| `savePersonalModelDraft(input)` | 草稿保存（可改名） | `basedOnRevision` 见下方差异 |
| `testModelConnectivity(input)` | 走 `mode-agent` 的 `provider/testModelConnectivity` | 未装配 agent 控制面 → 显式报错；失败一律返回 `{success:false,error:{message,code?}}` |
| `listRemoteModels(input)` | `provider-model-listing::fetch_remote_model_catalog` | 纯读；失败返回 `{success:false,message}` |

`onDidChange`：Personal 仓库 `on_did_change` 驱动，重建 View 后广播；监听者首订阅时才挂
仓库订阅（与 `model-selection` 同款）。

**与 TS 的三处已登记差异**（都不改变渲染层可见语义）：

1. **membership 在锁内构造**。TS 由 facade 在写之前从 snapshot 构造 `ProviderModelMembership`
   并断言 revision；Rust 的写入本就落在 `PersonalConfigRepository` 的跨进程锁内、从锁眼看
   到的 `PersonalLayer` 计算，因此频道直接按锁内层构造 membership 的 `personalRevision`，
   不存在「读到旧 revision 再写」的窗口（比 TS 更强，不是放宽）。
2. **`savePersonalModelDraft.basedOnRevision` 不做跨快照 CAS**。TS 比对 Registry revision；
   Rust 的读-改-写在锁内完成，`originalModelId → nextModelId` 的改名冲突由写面自身校验
   （`Provider 名称已存在` / `Model 已存在`）拒绝，不会静默覆盖。
3. **`waitForProviderOperations` 不存在**。TS 用它等 mutation tail 收敛；Rust 写入同步完成
   并立即返回新 View，无需等待。

为支持「校验失败不落盘」，`provider-config-repository` 新增
`PersonalConfigRepository::try_update`：变换返回 `Err` 时**不写文件、不推进
writeGeneration、不触发 onDidChange**（锁内失败必须能被反复重试且无副作用）。

**装配**：`ChannelAssemblyOptions.mode_builtin_provider_config_path` 提供 Builtin 路径
（同 `model-selection`）；Personal 路径 = `data_root/v2/provider_config.json`。缺 Builtin
路径时频道不可构造（留痕跳过，回落 Node），与 model-selection 同口径。频道已进
`service_channel_names()` 与 inprocess `default_owned_channels()`（35 个）。

## 6. 验收

- 阶段 0：`cargo test -p provider-config --lib exact_rule`。
- 阶段 4 频道契约（2026-11-23，`crates/host-channels/tests/provider_settings_channel.rs`，9 条）：
  View 投影可读且读面不落盘、create/delete 返回新 View、**校验失败不落盘**、
  `onDidChange` 只在真实写入后广播一次且退订后静默、连通性前置资格
  （不存在/未启用 → `success:false` + `code`，缺字段显式报错）、
  `resolveModelConfig` 无草稿时 inherited == effective 且问题路径带模型定位前缀、
  远端目录对不支持的 apiType 返回 `success:false` 且不写配置、未知方法拒绝。
- 仓库层：`cargo test -p provider-config-repository --test repository`（12 条，含
  `try_update_leaves_no_trace_when_the_transform_rejects`）。
- 全量：`cargo test -p host-channels`（33 个测试目标全绿）与
  `cargo clippy -p host-channels -p provider-config-repository -p mode-tauri --all-targets -- -D warnings`。
- 真机（待用户）：Provider 设置页读写/排序/启停、连通性测试、远端模型探测。
