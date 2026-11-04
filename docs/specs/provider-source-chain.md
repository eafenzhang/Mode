# Provider Source Chain（账号 Provider 事实链）

> 范围：`crates/provider-source-chain`。把「Provider 配置文件源」与「账号层 Overlay」串成
> Mode agent 可直接消费的 `ProviderSource`，对应 TS `AccountProviderService` +
> `createAccountProviderConfigSource` 的编排语义。
>
> 相关规范：`provider-config-source.md`（文件层）、`account-provider-overlay.md`（账号事实
> 投影与 revision）、`agent-process.md`（交付协议）、`provider-config.md`（模型/规则）。

## 1. 为什么需要这一层

`agent-process::ProviderConfigSync` 只做协议侧：读快照 → 串行 → 按 revision 去重 → 发
`provider/updateAccountConfig` → 校验回执。它刻意不碰账号事实。上游 TS 由
`AccountProviderService` 负责编排：

1. 读组合配置快照（Built-in + Personal）；
2. 用 **Built-in providers**（不是合并后的集合）作为“已配置 Provider”输入账号解析；
3. 解析失败时首次发布 fail-closed 快照，之后保留 last-known-good 并上报错误；
4. 解析成功时按 `createAccountProviderConfigSnapshot` 生成 revision 并发布。

本 crate 就是这一编排的 Rust 实现，因此它依赖两个已经 Rust 化的域：文件源与账号 Overlay。

## 2. 输入

| 参数 | 含义 |
| --- | --- |
| `bundled_path` | 随包 Mode Built-in Release 路径 |
| `active_path` | 可选 Active Release 路径（优先于 bundled） |
| `personal_path` | Personal `provider_config.json` 路径 |
| `options: O` | 账号事实注入点，实现 `AccountProviderOverlayOptions`（设置、账号身份、Coding Plan Key、Family 可用性） |

`reasons` 的来源与 TS 一致：首次读取为 `start`，显式刷新为调用方给出的原因，
配置源变化为 `config:<reason>`（空原因记作 `config:changed`）。其中包含凭据刷新原因时，
Overlay 会强制刷新 Coding Plan Key。

## 3. 输出

`agent_process::AccountProviderConfigSnapshot`：

- `revision`：`account:${JSON.stringify([basedOn, providers.toJSON(), states])}`
- `based_on_mode_builtin_revision`：`mode-builtin:<release>:<sourceSha256>`
- `providers`：`providerId -> config.toJSON()` 的对象（**不是** rule 数组；与 TS 线格式一致）
- `states`：`providerId -> AccountProviderState`，无状态时为空对象

## 4. 失败语义

| 情形 | 行为 |
| --- | --- |
| 配置文件读取失败（bundled/active 均不可用等） | 返回错误，不发布快照 |
| 首次账号解析失败 | 发布 fail-closed 快照（账号 Provider `entitled=false`、无 states），不返回错误 |
| 非首次账号解析失败 | 保留 last-known-good，返回错误（调用方决定是否重试） |
| 解析成功但 revision 与上次相同 | 正常返回；去重由 `ProviderConfigSync` 按 workspace/revison 处理 |

fail-closed 只用 Built-in 里 `access.type = zhipu-account` 的 Provider，且不改动其它 Provider。
这与 TS `createFailClosedAccountProviderConfigSnapshot` 一致。

## 5. 状态所有权与并发

`AccountProviderService` 是唯一的编排状态所有者：

- `read()` 缓存首份快照；无快照但有 in-flight 轮次时共享该轮结果，不会重复解析。有快照
  后直接返回缓存，文件变化由配置源订阅触发后台 `refresh("config:*")`。
- `refresh(reason)` 递增 `requested_generation` 并登记 waiter；只有覆盖该 generation 的
  轮次能回应。轮次开始后到达的请求留给下一轮。
- 每轮解析前读一次配置、解析后再读一次复核：复核 revision 变化或期间出现新请求时整轮
  丢弃，补 `superseded-resolution` 后重跑，绝不短暂发布过期快照。
- 首次解析失败发布 fail-closed；之后失败保留 last-known-good 并回绝本 generation 的
  waiter。失败轮期间登记的 waiter 由后续轮次回应（`finishRefresh` 语义）。
- `onDidChange` 只在已有旧快照且 revision 变化时通知，reason 是本轮 reasons 的
  `join(",")`；`onDidRefreshError` 每轮失败都会通知（含 fail-closed 轮）。
- `dispose()` 退订配置源、拒绝所有等待中的 refresh、清空监听器；in-flight 轮次自行结束。

`ProviderChain`（同步单轮解析）保留为测试/诊断辅助；生产交付入口是
`AgentProviderSource` → `AccountProviderService` → `to_agent_snapshot`。配置文件读取与
revision 去重仍分属配置源与 `ProviderConfigSync`，服务不重复实现。

## 6. 验收场景

1. Built-in 含 `zai-start`（start-plan）与 `zai-individual`（individual-coding-plan），设置选
   `zai` + `individual-coding-plan`，身份存在，可用性分别为 available / available(models)：
   - revision 以 `account:` 开头，`basedOnModeBuiltinRevision` 等于配置快照的
     `mode_builtin_revision`；
   - `providers.zai-start.access` 与 `providers.zai-individual.access` 为账号投影；
   - `states` 含 `availability/entitled/current` 等字段。
2. 连续两次 `read()`（文件不变）得到同一 revision。
3. 首次 Overlay 失败：返回 fail-closed 快照，账号 Provider `entitled=false`，`states` 为空。
4. 已有成功快照后 Overlay 失败：返回错误，下一次成功读取仍能发布新 revision。
5. 端到端：`AgentControlClient::sync_account_provider_config` 发出的
   `provider/updateAccountConfig` 参数与本链输出一致；重复调用返回 `Unchanged`。
6. 并发首读共享同一轮（Resolver 只调用一次）；一轮开始后调用 `refresh("explicit")`
   必须再跑一轮后才回应。
7. 解析期间配置 revision 变化：整轮作废，最终快照的 `basedOnModeBuiltinRevision` 为新
   revision，Resolver 至少调用两次。
8. 配置源通知触发后台刷新；成功后 revision 变化才触发 `onDidChange`，首份快照不触发。
9. `dispose()` 后等待中的 refresh 返回 `AccountProviderService 已 dispose`。
