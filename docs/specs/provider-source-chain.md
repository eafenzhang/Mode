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

`reasons` 由调用方传入（`start`、`config:*`、`explicit` 等），语义与 TS 一致：其中包含
凭据刷新原因时，Overlay 会强制刷新 Coding Plan Key。

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

- `ProviderChain` 持有 last-known-good（上一份成功或 fail-closed 快照），一个实例对应一个
  逻辑数据根；`resolve` 串行执行（`&mut self`）。
- `AgentProviderSource` 用异步互斥包住 chain，实现 `ProviderSource`，因此同一 Source 的被
  多个同步调用并发读取时不会交错推进 last-known-good。
- 不缓存配置文件读取结果；每次 `read()` 都重新读文件层，避免磁盘变更被吞掉。revision
  去重只在 `ProviderConfigSync` 做。

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
