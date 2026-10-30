# Account Provider Overlay（Rust）

> 范围：`@mode/provider` 与 `packages/services/src/model-provider` 中
> **账号层 Provider Overlay** 的 Rust 移植。
> 真源（clean-room，只读本仓库 TS）：
> `packages/provider/src/account-provider-service.ts`、
> `packages/provider/src/account-provider-resolution.ts`、
> `packages/services/src/model-provider/accountProviderConnectionResolver.ts`、
> `packages/provider/src/sources.ts`、
> `packages/provider/src/account-provider-state.ts`、
> `packages/shared/src/{model-provider-family,provider-family-connection-selection,account-provider-state}.ts`。
>
> Rust crate：`crates/account-provider-overlay`；
> 对拍脚本：`scripts/generate-account-provider-overlay-fixtures.ts`
> （`pnpm account-provider-overlay:fixtures`）。

## 1. 范围与非范围

### 本轮做

- 账号连接设置为领域值：`AccountProviderConnectionSettings`
  （`providerFamilyDomain` / `selections` / `unresolvedFamilies`）。
- Coding Plan 可用性结果与 Connection Result 的纯类型。
- `resolve_account_provider_connections`：family 分组、identity 去重缓存、
  `unresolvedFamilies`、无身份、API Key、off-peak 选择匹配、team scope、
  `previousScopes` / `resetPrevious`、`unavailableReason`。
- `resolve_account_provider_configs`：把连接结果投影为第三层 Provider Config
  （只写 `access` / `entitled` / `builtinModelIds`，不写 token/key）。
- `AccountProviderState(s)` 生成规则与 `unknown` 的 last-known-good 继承。
- `create_account_provider_config_snapshot`：revision 与 states 缺省语义。
- 函数组合 `create_account_provider_config_resolver(options)`。

### 本轮不做（后续切片）

- 真实凭据、OAuth、网络可用性查询、`ApiClient`。
- `AccountProviderService` 的刷新编排、监听、waiter、并发 generation。
- registry / resolver 的非账号部分、model listing、频道/所有权。
- `resolveCurrentAccountAccess`（Active Model 动态访问）与 UI 侧消费。
- 适配 `agent-process` 的 ProviderSource（下一片）。

## 2. 输入与输出

### 2.1 注入点

异步回调以 trait 表达，与 TS options 一一对应：

| trait 方法 | TS | 说明 |
| --- | --- | --- |
| `read_settings` | `readSettings()` | 每轮解析开头读取一次，发布前再读一次做 CAS |
| `load_account_identity` | `loadAccountIdentity(family)` | 原始值；解析层负责 `trim` 与空串归一 |
| `load_coding_plan_api_key` | `loadCodingPlanApiKey(providerId, family, identity, forceRefresh)` | 失败（`Err`）在解析层折叠为 `None` |
| `resolve_family_availability` | `resolveFamilyAvailability({family, providers, selections})` | 返回 `providerId -> result`，缺项按 `unknown` |

### 2.2 Resolve 输入

```text
configRevision（本轮不参与账号判定，仅随快照发布）
configuredProviders : ProviderConfigMap
previousProviders   : ProviderConfigMap
previousStates?     : AccountProviderStates
reasons[]           : string
```

### 2.3 Resolve 输出

```text
providers : ProviderConfigMap     // 只含 access/builtinModelIds
states    : AccountProviderStates // 键顺序 = connections 顺序
```

## 3. 分支语义（与 TS 同规则）

### 3.1 Family 分组

固定按 `zai`、`bigmodel` 顺序。某 family 的候选是 `configuredProviders`
中同时满足：

- `access.type === "zhipu-account"`
- `access.accountType === family`
- `access.mode` 存在且不是 `off-peak`

候选为空则跳过该 family。

### 3.2 未决身份 / 无身份

- `unresolvedFamilies` 含该 family 且 `mode !== "start-plan"` 时：
  该 Provider 记 `unknown`，不参与查询；`start-plan` 不受影响。
- 过滤后无查询项 → 跳过该 family。
- `loadAccountIdentity(family)` 结果 `trim` 后为空 → 所有查询项
  `unavailable` 且 `reason = coding_plan_not_connected`。

同一轮内 identity 按 family 缓存一次（ts 侧缓存 Promise）。解析末尾用
**原始注入**重新读取每个已缓存 family，结果变化则整轮失败（不推进
`previousScopes`），错误原文：

```
账号查询期间连接或身份发生变化，丢弃过期结果
```

设置同样在发布前重新读取并做结构比较，变化时同上失败。identity 比较先于
settings 读取，identity 已变化时不再调用 `readSettings`。

### 3.3 API Key

- `team-coding-plan` 不读取 API Key，固定传 `null`。
- 其它 plan 调用 `loadCodingPlanApiKey`；抛错（`Err`）折叠为 `null`。
- `forceRefresh` 由 reasons 决定：任一 reason 含 `oauth-callback`，或
  `split(":").at(-1) === "oauth-login-entitlement"`。

### 3.4 连接结果

对每个 `zhipu-account` Provider，按 configured 顺序：

- `accountType` 或 `mode` 缺失 → `{status: "unavailable"}`（无 reason、
  无 current/connectionKey）。
- `scope = JSON.stringify([identity, teamScope])`；`teamScope` 仅当
  `mode === "team-coding-plan"` 且当前选择也是 team 时存在，顺序为
  `[organizationId, projectId, productId]`。
- `resetPrevious = previousScopes.has(id) && previousScopes.get(id) !== scope`。
  `previousScopes` 仅在整轮成功发布后整体替换。
- `off-peak`：
  - 选择 kind 必须是 individual 或 team；
  - `providerFamilyDomain === accountType`；
  - configured 中存在同 family、同 mode 的 Provider，其 availability 为
    `available`；
  - 三者同时成立 → `available`，否则 `unavailable`；只发布
    `providerId` / `status`（无 current、connectionKey、resetPrevious）。
- 其它 mode：
  - `status` = availability kind；
  - `unavailable` 时带 `unavailableReason`（映射见 §3.5）；
  - `current = providerFamilyDomain === accountType &&`
    `mode === "start-plan" ? identity !== null : selection.kind === mode`；
  - `connectionKey = sha256(JSON.stringify([identity, accountType,`
    `mode === "start-plan" ? {kind:"start-plan"} : selection ?? null]))`；
  - availability 为 `available` 时仅在存在 `models` 时携带；`pending`
    携带 `models` 与 `effectiveAt`；
  - `resetPrevious === true` 时携带该字段。

### 3.5 原因映射

| Coding Plan | Account State |
| --- | --- |
| `coding_plan_not_authenticated` | `not-authenticated` |
| `coding_plan_not_connected` | `not-connected` |
| `coding_plan_auth_failed` | `credential-failed` |
| `coding_plan_not_entitled` | `not-entitled` |

## 4. Config Overlay

`resolve_account_provider_configs`：

1. 建立连接索引；重复 provider、未配置 provider、非 Account provider 均失败：
   - `重复 Account Provider 连接结果: <id>`
   - `Account 连接指向未配置 Provider: <id>`
   - `Account 连接指向非 Account Provider: <id>`
2. 按 configured 顺序遍历 `zhipu-account`：
   - `available` / `pending`：
     - `start-plan` → `access = {type:"zhipu-account", entitled:
       status==="available"}`，`builtinModelIds = trim + 去空`（缺失即 `[]`）；
     - 其它 → 只写 `access.entitled`。
   - `unavailable` → `access.entitled = false`。
   - `unknown` → `resetPrevious` 为真或没有 previous 时
     `access.entitled = false`，否则沿用 previous 的完整 config。
3. 输出 Map 的键顺序 = configured 顺序；规则只保留 `providerId` + `config`。

永不写入 token / API Key / header / 账号身份。

## 5. States

对每个 connection（顺序不变）：

- `previous`：`resetPrevious` 时为空，否则取 `previousStates[id]`。
- `availability`：`unknown` 且存在 previous → previous.availability，
  否则 connection.status。
- `entitled`：输出 providers 中该 id 的 `access.type === "zhipu-account" &&
  entitled === true`。
- `unavailableReason`：`unknown` 且有 previous → previous 的值；
  `unavailable` → connection 的值；其它不写。
- `current`：connection.current；`unknown` 时缺失则继承 previous.current。
- `connectionKey`：**总是**取 connection 的值（不继承 previous）。
- `effectiveAt`：connection 的值；`unknown` 时缺失则继承 previous.effectiveAt。

序列化键顺序固定为 `availability, entitled, unavailableReason?, current?,
connectionKey?, effectiveAt?`。TS 侧由字面量插入序得到相同顺序；人工构造的
previousStates 若使用非规范键序，会被归一为本顺序（已知差异 §7）。

## 6. Snapshot 与 revision

```text
revision = `account:${JSON.stringify([basedOnModeBuiltinRevision,
                                     providers.toJSON(), states])}`
```

- `states === undefined` 时数组第三项仍序列化为 `null`，但返回对象不携带
  `states` 字段。
- `states` 存在（包括 `{}`）时写入对象，并参与 revision。

Rust 用同一公式序列化：`providers.to_json()` 的数组顺序、规则字段顺序、
states 键顺序均与 TS 一致。

## 7. 与 TS 的已知差异 / 风险

- 身份缓存按 family 缓存**已解析结果**而非 Promise；注入回调抛错时，Rust
  在首次等待点传播，TS 缓存 rejected Promise。正常路径一致。
- `normalizeModelIds` 使用 Rust `trim`（Unicode White_Space）；JS `trim`
  还会去掉 BOM（U+FEFF）。两者对 ASCII 空白一致。
- 人工构造的 `previousStates` 如果键序不是规范顺序，输出会被归一为规范顺序
  （TS 保留插入顺序）。`previousStates` 由本 resolver 上一轮产出时不存在差异。
- `AccountProviderService` 的刷新循环、并发 generation、waiter、fail-closed
  首次快照不在本轮；仅提供纯解析函数与 snapshot 构造。
- 真实 OAuth/网络/凭据、model listing、registry 非账号部分未移植。

## 8. 对拍

`pnpm account-provider-overlay:fixtures` 用真实 TS：

```text
createAccountProviderConfigResolver(createAccountProviderConnectionResolver(options))
```

驱动确定性桩（settings / identity / API key / availability），输出：

- `scenarios[].steps[]`：stateful resolver（同一 scenario 内 `previousScopes`
  连续），每步包含 configured/previous/settings/桩序列/reasons，以及
  connections、providers、states、revision、调用日志的规范 JSON 原文。
- `configCases[]`：直接调用真实 `resolveAccountProviderConfigs` 的覆盖
  （start-plan 模型归一、unknown 继承/重置、三类索引错误）。
- `snapshotCases[]`：`createAccountProviderConfigSnapshot` 的 states
  缺省/存在与 revision 原文。

Rust 侧用相同输入与桩重放，逐字节比较规范 JSON 与错误原文。

## 9. 下一片

1. 刷新编排：`AccountProviderService` generation / waiter / last-known-good /
   首次 fail-closed。
2. 真实凭据与可用性源：OAuth 身份、Coding Plan API Key、网络查询与缓存。
3. 适配 `agent-process` 的 ProviderSource，把账号层接到 Config Source 链。
