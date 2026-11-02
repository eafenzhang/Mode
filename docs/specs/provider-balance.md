# Spec：外部 Provider 余额识别与解析（Rust `provider-balance`）

状态：2026-11-02 第三版（增加 Provider 设置视图目标解析）。
真源：`packages/services/src/usage-stats/providers/providerBalanceSpecs.ts`、
`providerBalanceProvider.ts`。

## 1. 范围

| Rust 入口                       | TS 对应                              |
| ------------------------------- | ------------------------------------ |
| `resolve_provider_balance`      | `resolveProviderBalanceProvider`     |
| `parse_provider_balance`        | `ResolvedProviderBalanceProvider.parse` |
| `ProviderBalanceProvider::get_snapshot` | `ProviderBalanceProvider.getSnapshot` |
| `provider_balance_target_from_view`    | `createProviderBalanceTargetResolver` |

纯函数：按 Provider 配置的 `api.baseUrl` 主机名识别供应商，解析固定余额接口的响应；
不发起网络请求、不读设置。规范中的金额字段均为 JSON number / numeric string。

## 2. 识别规则

| 主机名（含子域）                      | 查询地址                                          |
| ------------------------------------- | ------------------------------------------------- |
| `api.deepseek.com`                    | `https://api.deepseek.com/user/balance`           |
| `api.moonshot.cn` / `api.moonshot.ai` | `https://<host>/v1/users/me/balance`              |
| `api.siliconflow.cn` / `.com`         | `https://<host>/v1/user/info`                     |
| `api.stepfun.com` / `.ai`             | `https://<host>/v1/accounts`                      |
| `openrouter.ai`                       | `https://openrouter.ai/api/v1/credits`            |
| `api.novita.ai`                       | `https://api.novita.ai/v3/user/balance`           |

主机名按 URL 解析后小写；`host === domain` 或 `host.endsWith("." + domain)`。
`null`/空白/非法 URL/未识别主机返回 `None`。识别时会记录小写 host，Moonshot /
SiliconFlow 的币种由它决定（`.ai` / `.com` 为 USD，其余 CNY）。

## 3. 解析规则

- 通用：只接受对象根；`data` 对象存在时优先取 `data`（Moonshot/SiliconFlow/OpenRouter）；
  数字字段接受有限 number 或非空 numeric string（含 `0x/0b/0o`）；
  布尔/字符串只接受对应类型；空字符串与未 trim 的字符串按缺失处理。
- 条目形状：`label`、`unit`（缺省 null）、`remaining`（缺省 null）、`total`/`used`
  （缺省时不出现，显式 null 时出现）、`isAvailable`（缺省 true）。
- DeepSeek：`is_available` 缺省 true；`balance_infos[]` 逐条 `currency`（缺省 CNY），
  金额 `total_balance`。
- Moonshot：`available_balance` / `voucher_balance`；`isAvailable = remaining > 0`
  （remaining 为空时 true）。
- SiliconFlow：`totalBalance ?? balance`；`chargeBalance` 为 total。
- StepFun：`balance` / `total_cash_balance`，固定 CNY。
- OpenRouter：`total_credits` - (`total_usage` ?? 0)；两个字段都输出（可为 null）。
- Novita：`availableBalance` / `cashBalance` 均除以 10000，固定 USD。

## 3.1 快照编排

`ProviderBalanceProvider` 通过注入的 `ProviderBalanceTargetResolver` 与
`ProviderBalanceFetcher` 取目标与响应，自身不读设置、不发网络请求：

1. `generatedAt` 由注入时钟生成；`providerId` trim 后为空或无 resolver → `unsupported`。
2. 目标解析失败/返回 null → `unsupported`；providerName 取目标名 trim 后的非空值，
   否则回退 providerId。
3. 未识别 baseUrl → `unsupported`；API Key trim 后为空 → `not_configured`。
4. 取回失败：HTTP 401/403 → `unauthorized`；其它错误 → `error` + `request_failed`。
5. 解析结果为空 → `error` + `empty_response`；否则 `ok` + balances。
6. 所有非 ok 快照的 `balances` 为空数组，`message` 只在 error 时出现。

## 3.2 目标解析

从 Provider Settings View（JSON）按 `providerId` 精确查找；只有
`effectiveConfig.access.type === "api-key"` 参与余额查询，账号型 Provider 返回 `None`。
`providerName` trim 后为空回退 providerId；`baseUrl` 取
`effectiveConfig.api.baseUrl`（缺失/null 为 null）；`apiKey` 仅接受字符串（原样保留，
不 trim），否则 null。

## 4. 已知差异

1. **数字字符串**：`Number("1_000")`、`Number(" 12px")` 等非法形式两边都返回缺失；
   已实现十进制/十六进制/二进制/八进制与前缀符号，其余极端格式不复刻。
2. **URL 解析**：使用 `url` crate；非法/相对地址一律视为未识别，与 `new URL` 失败分支一致。
3. 输出是 `serde` 序列化结果；对象键顺序不影响语义（当前顺序与 TS 字面量一致）。

## 5. 验收

- `pnpm provider-balance:fixtures` → `crates/provider-balance/fixtures/provider-balance.json`
  （13 个 baseUrl 识别 + 14 组响应解析：多币种、字符串金额、data 包装、负数/空余额、
  total/used 缺省与 null、Novita 单位换算、未识别供应商与非法 URL；14 组快照编排：
  空 id/无 resolver/解析失败、未知 baseUrl、空 Key、401/403/500/网络错误、
  成功与空响应、providerName 回退；11 组目标解析：缺失 provider、账号型/缺失 access、
  空名称、null baseUrl、缺失 api、null/数字 apiKey、多 provider、大小写不匹配）。
- `cargo test -p provider-balance`（逐项深比较 + null/空白 baseUrl）。
- `cargo clippy -p provider-balance --all-targets -- -D warnings`。
