# Spec：`provider-settings` 频道（Rust，分阶段迁移）

状态：2026-11-23 阶段 0/1 已落（精确规则访问器 + View 投影 builder）；写入/频道待后续。

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

## 3. 阶段 2：写入面（待做）

`createPersonalProvider` / `savePersonalProviderOverlay` / `deletePersonalProvider` /
`reorderPersonalProviders` / `reorderPersonalModels` / `addPersonalModel` /
`renamePersonalModel` / `deletePersonalModel` / `savePersonalModelDraft` /
`setPersonalModelEnabled`：
- 全部经 `PersonalConfigRepository::update`（跨进程锁 + 原子写）；
- 校验复用 `provider-config` 的完整规则 schema（TS `SettingsMutationTarget` 同义）；
- 成功后重新读快照并返回新 View（revision+1）。

## 4. 阶段 3：连通性 / 远端目录（待做，有外部依赖）

- `testModelConnectivity`：Rust `mode-agent` 尚未实现该命令，需先补
  `testModelConnectivity`（agent 协议往返）或保留 Node 代理。
- `listRemoteModels`：`crates/provider-model-listing` 已有 build/parse 纯逻辑，
  需补 reqwest executor + 草稿连接信息组装。

## 5. 阶段 4：频道与装配（待做）

- `host-channels::provider_settings` 频道同名映射；`onDidChange` 由 Personal 仓库
  通知驱动（View 广播）。
- 装配：Builtin 路径（同 model-selection 注入）+ Personal 仓库；进入 inprocess
  默认集前必须先完成阶段 1–3，避免半个频道。

## 6. 验收

- 阶段 0：`cargo test -p provider-config --lib exact_rule`。
- 后续阶段：View 投影 TS fixture 对拍 + mutation 幂等/CAS 用例 + 频道契约用例；
  真机：Provider 设置页读写/排序/启停、连通性测试、远端模型探测。
- `cargo clippy -p provider-config --all-targets -- -D warnings`。
