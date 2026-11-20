# Spec：usage-stats 频道（Rust `host-channels::usage_stats`）

状态：2026-11-21 首版（LT-15 P4 服务面接线）。
真源：`packages/services/src/usage-stats/usageStats.ts` 的
`IUsageStatsService`（`ServiceChannels.UsageStats = "usage-stats"`）。

## 1. 范围

| Rust 方法 | TS 方法 | 说明 |
| --- | --- | --- |
| `getProviderBalanceSnapshot` | `getProviderBalanceSnapshot` | 外部 api-key Provider 的账户余额快照 |

官方套餐/额度（`codingPlan`/`account`）链路已随去智谱化整体下线
（TS `assertOfficialServiceRemoved` 恒抛），**不接线**；其余 `IUsageStatsService`
方法（app usage 快照、entitlement 等）待后续切片（agent-server 的
`getAppUsageStats` 走 v4 protocol 直查 SQLite，不在本频道）。

## 2. `getProviderBalanceSnapshot`

- 请求：`{ providerId?: string }`（camelCase）。缺省/undefined/空串与 TS
  `request.providerId?.trim() ?? ""` 同语义：返回 unsupported 快照，**不是参数错误**。
- 响应：`ProviderBalanceSnapshot` camelCase JSON（`providerId/providerName/
  generatedAt/status/message?/balances`）。
- 快照状态机与供应商识别见 `docs/specs/provider-balance.md`
  （unsupported / not_configured / unauthorized / error(+`request_failed`|
  `invalid_response`|`empty_response`) / ok）。

## 3. 装配

- **标准**：`UsageStatsChannel::with_personal_layer(Arc<PersonalConfigRepository>)`
  —— Personal 层读层（1s 轮询 LKG）+ `ReqwestBalanceFetcher`（15s 超时）。
  personal config 路径：`MODE_PERSONAL_PROVIDER_CONFIG_FILE` env 优先，回退
  `<data_root>/v2/provider_config.json`（与 agent_runtime 的 ProviderPaths
  fallback 同口径）。
- **降级**：repository 构造失败或 reqwest client 构造失败 →
  `UsageStatsChannel::default_channel()`（无 resolver，一律 unsupported）并留
  shell 日志；**不阻塞频道注册**（渲染层拿到的是诚实的 unsupported，不是 RPC 错误）。
- **TS 缺省语义**：`default_provider_balance()`（无 resolver）对应 TS
  `createUsageStatsService` 不传 `resolveProviderBalanceTarget` 的一律
  `unsupported` 面，保证未装配环境不产生意外网络请求。

## 4. 已知差异

1. **凭据来源**：TS 经 `ProviderSettingsView.effectiveConfig`（builtin 模板 +
   personal 覆盖合成）；Rust 本切片从 `PersonalLayer` 直读（外部 api-key Provider
   由用户自配、落在 personal 层），builtin 模板合并待 ProviderSettingsView 移植。
2. **方法面**：TS `IUsageStatsService` 的其余方法未迁本频道；app usage 已由
   agent-server `getAppUsageStats`（v4 protocol）覆盖，语义面不缺。

## 5. 验收

- `cargo test -p provider-balance`：识别/解析/状态机/HTTP fetcher（mock server）。
- `cargo test -p host-channels --test usage_stats_channel`：ok 快照、unsupported
  （未知 provider / 缺省 providerId / default 装配）、not_configured（空 key）、
  未识别 baseUrl、unknown command、频道名漂移守卫（`channel_names.rs`）。
- 壳内注册：`apps/desktop-tauri/src-tauri/src/inprocess.rs`
  `is_owned_channel` + `default_owned_channels` 含 `usage-stats`。
