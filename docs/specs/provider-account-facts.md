# Spec：账号 Provider 事实注入（Rust `provider-account-facts`）

状态：2026-11-05 第一版。
真源：`packages/services/src/model-provider/accountProviderConnectionResolver.ts` 的生产
options 与 `codingPlanProviderAvailability.ts` 的对照逻辑。

## 1. 范围

| Rust 入口 | TS 对应 |
| --- | --- |
| `ProviderAccountFacts::read_settings` | `readSettings` |
| `ProviderAccountFacts::load_account_identity` | `loadAccountIdentity` |
| `ProviderAccountFacts::load_coding_plan_api_key` | `loadCodingPlanApiKey`（缓存分支） |
| `ProviderAccountFacts::resolve_family_availability` | `resolveFamilyAvailability` |
| `ProviderAccountFacts::resolve_runtime_api_key` | `interaction/requestProviderRuntimeHeaders` 的凭据解析 |
| `AccountSettingsPort` / `AccountCredentialPort` / `CodingPlanUrlPort` | 宿主注入点 |
| `EnvCodingPlanUrls` | `buildRuntimeZaiBusinessUrl` / `buildBigModelApiUrl` / `buildZaiStartPlanBalanceUrl` |

组合成 `account_provider_overlay::AccountProviderOverlayOptions`，可直接交给
`provider-source-chain::AgentProviderSource`。

## 2. 行为

- 设置：端口返回已解析的 `AccountProviderConnectionSettings`。
- 身份：`oauth:<zai|bigmodel>:user_info` JSON 的 `id`（trim 非空）。
- Coding Plan Key：优先读
  `account-provider:coding-plan:<providerId>:account:<encodeURIComponent(identity)>:api-key`；
  缓存缺失或 `force_refresh` 时，用 `oauth:<provider>:access_token` 经
  `CodingPlanKeyExchange` 兑换并回写缓存（未注入兑换端口则返回 `None`）。
- 可用性：
  - Start Plan：优先 `zcodejwttoken`（`Bearer <jwt>`），否则 provider apiKey；
    调 `start_plan_balance_url()`；
  - Individual：provider apiKey（去掉 `Bearer ` 前缀）调 family 对应订阅列表 URL；
  - Team：从 settings 的 family 选择取 `organizationId/projectId`，经
    `team_host` 做 customerInfo 项目校验 + 团队订阅详情；未选中团队或缺少 access token
    按 `Unknown` / `not_connected`；
  - 空凭据由 `coding-plan-http` 映射为 `not_authenticated` / `unknown`。
- URL：`EnvCodingPlanUrls` 按 `MODE_BASE_URL`/`MODE_ENDPOINT_ORIGIN`、
  `ZAI_BUSINESS_BASE_URL`、`BIGMODEL_API_BASE_URL` 解析并归一化 origin；
  Start Plan balance 走 `/api/v1/mode-plan/billing/balance?app_version=…`。
- 壳侧（`agent_runtime.rs`）已用 `ReqwestCodingPlanHttp` + settings/credential 端口
  装配为 `AgentProviderSource`；构造失败回退文件层。
- `AccountFactsRuntime` 订阅 `PersonalConfigRepository` 1s 轮询：
  `updated`/`poll-changed` → `refresh("personal:<reason>")` +
  `refresh_provider_config()` 重交付；`poll-error` 保留 LKG。

## 2b. Provider runtime headers

`resolve_runtime_api_key(providerId, accountAccess)`：
Start Plan → `zcodejwttoken`；Individual/Off-Peak → 缓存 Key 或 OAuth 兑换；
Team → settings 团队选择的 org/project + OAuth token → 团队项目 Key。
壳侧通过 `provider_runtime_headers_handler` 在注入运行时上延迟应答。

## 3. 差异

1. 兑换端口未注入时 `force_refresh` 只跳过缓存；注入后走
   `account-api-key-resolver`（见 `docs/specs/account-api-key-resolver.md`）。
2. 团队权益仍为 `Unknown`（需要团队 API Key 组合，后续接入）。
3. 设置端口返回已解析类型，不在此 crate 读取 settings 文件。

## 4. 验收

- `cargo test -p provider-account-facts`：10 条（设置/身份、Key 编码与
  force-refresh、OAuth→Key 兑换、个人可用性、Start Plan JWT、缺 Key、
  encodeURIComponent、URL/env、缺 Start Plan 凭据）。
