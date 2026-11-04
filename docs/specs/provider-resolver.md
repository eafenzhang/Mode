# Spec：Provider 配置解析（Rust `provider-resolver`）

状态：2026-11-04 第一版。
真源：`packages/provider/src/resolver.ts`、`packages/provider/src/owned-order.ts`。

## 1. 范围

| Rust 入口 | TS 对应 |
| --- | --- |
| `ProviderConfigResolver::resolve` | `ProviderConfigResolver.resolve` |
| `ProviderConfigResolverInput` | `ProviderConfigResolverInput` |
| `ResolvedProvider` / `ResolvedProviderModel` | 同名类型 |
| `RegistryProvider` / `RegistryModel` | `Provider` / `ProviderModel`（Registry 视图） |
| `ProviderConfigResolution` | `ProviderConfigResolution` |
| `resolve_owned_order`（私有） | `resolveOwnedOrder` |

只做纯计算：输入四层配置（Built-in、Personal、Account Overlay、模型规则）与
账号状态，输出 effective 配置、逐 Provider 的模型候选、Registry 子集与 issues。
HTTP、账号事实源、UI 编排与序列化助手（`serializeRegistry*`）不迁。

## 2. 行为

1. **Account Overlay**：只作用于 Built-in 已存在的 Provider；覆盖层先 `withoutGroup`
   再 overlay（分组归属永远来自 Built-in 基础层）。
2. **模板施加**：Built-in 与「非 Built-in 的 Personal」按各自 `templateId` 取模板
   `template.config.overlay(concrete)`；Personal 与 Built-in 同 id 时先 `withoutGroup`。
3. **顺序**：`zai-family` / `bigmodel-family` 永远最前；其余走 `resolveOwnedOrder`：
   未排序 Built-in → 用户顺序 → 未排序 Personal。
4. **enabled**：`zhipu-account` 恒为启用（旧覆盖值不能让无开关账号永久失效）；
   其余 `enabled !== false`。
5. **executable** = enabled ∧ accessEntitled（非账号或 `entitled === true`）
   ∧ accountCurrent（`state.current !== false`）∧ providerIssues 为空
   ∧ modelEnabled ∧ modelIssues 为空。
6. **selectable** = executable ∧ `visibility !== "hidden"`。
7. **Registry 子集**：仅当 providerIssues 为空且存在 executable 模型时发布；
   模型按 executable 过滤。
8. **issues**：Provider 级（含 `missing-template`）先收集，随后逐模型
   `models.<modelId>` 前缀的校验问题。

## 3. 已知差异

1. TS 用 zod `.strict()` 校验；Rust 复用 `validate_complete_at` /
   `validate_complete`，模型 issue 路径由本 crate 手工加
   `providers.<id>.models.<modelId>` 前缀。
2. 规则正则只支持 Rust `regex` 文法；JS-only 语法（lookahead/backref）判不命中
   （见 `provider-config` 的 resolve 说明）。
3. 输出为结构化类型；TS 的 `serializeRegistryProviderConfig` /
   `serializeRegistryModelConfig` 等序列化助手未迁（调用方按 wire 形状自行序列化）。
4. `RegistryProvider.config` 用 `ProviderConfig` 承载（已过完整校验的同一份配置），
   不引入独立的强类型别名。

## 4. 验收

- `cargo test -p provider-resolver`：7 条契约用例——账号覆盖仅限 Built-in 且去分组、
  模板叠加与 templateConfig 透出、missing-template 阻断 Registry、family 优先与
  owned 顺序、账号恒启用 + hidden 不可选、`current=false` 阻断 executable、
  Personal 与 Built-in 同 id 时去分组且只出现一次。
- `cargo clippy -p provider-resolver --all-targets -- -D warnings`。
