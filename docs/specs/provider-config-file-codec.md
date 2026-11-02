# Spec：Personal Provider Config 文件 codec（Rust `provider-config-file-codec`）

状态：2026-11-02 第一版。
真源：`packages/provider-node/src/provider-config-file-codec.ts`。

## 1. 范围

| Rust 入口 | TS 对应 |
| --- | --- |
| `decode_provider_config_file` | `decodeProviderConfigFile` |
| `encode_provider_config_file` | `encodeProviderConfigFile` |
| `ProviderConfigLayerUpdate` | 同名接口 |
| `ProviderConfigCodecError` | `UnsupportedProviderConfigVersionError` + schema 错误 |

只做 JSON 编解码；文件读取、原子写、锁与 polling 由 Repository 适配层负责。

## 2. 解码

- `schemaVersion`：缺失/非非负整数/负数 → UnsupportedVersion；>1 → 高于当前版本；
  0 → 缺少 0→1 迁移器。错误消息与 TS 一致。
- 顶层严格只允许 `schemaVersion/config`；config 严格只允许
  `providerOrder/providerConfigRules/modelConfigRules/defaultModelSelection`。
- `providerOrder` 可选，必须是非空字符串数组。
- `providerConfigRules` 交给 `parse_personal_provider_config_map`；
  `modelConfigRules` 先做旧手动规则归一化，再交给
  `parse_personal_model_config_rules`；任一失败即整体失败。
- `defaultModelSelection` 可选，使用 `ModelSelection` 严格解析（trim、未知键拒绝）。

## 3. 旧手动规则归一化

当 manualProviderModelRules[].config 无法按当前手动 schema 解析时：

1. 仅接受结构键在旧完整/旧可编辑集合内的 config（root/properties/inputFormat/
   outputFormat/optionSpecs/reasoningLevel/maxOutputTokens 均有白名单）；
2. 提取当前可编辑字段：enabled、contextWindow、supportsJsonSchemaOutput、
   supportsNativeWebSearch、supportsMidConversationSystem、inputFormat 的
   supportsImage/Video/Pdf、reasoningLevel 整体、maxOutputTokens.max；
3. 提取后必须通过当前手动 schema，否则保留原样并让整体解析失败。

未知键不会被静默丢弃；旧完整形状独有的 requiresMfjsToolSchema/outputFormat/
supportsToolCall/文本/音频能力会在写回时消失。

## 4. 编码

输出 `{schemaVersion:1, config:{providerOrder?, providerConfigRules:{providerRules},
modelConfigRules, defaultModelSelection?}}`。provider/model 规则与 selection 使用各自
规范化序列化；`providerOrder`/`defaultModelSelection` 缺失时省略。

## 5. 已知差异

1. zod 的 schema issue 列表不逐条复刻；Rust 返回单条 Invalid 字符串。
2. 旧完整形状中会被提取丢弃的 `maxOutputTokens.map` 不再二次校验；当前手动 schema
   仍校验保留字段。夹具覆盖非法 reasoningLevel map。
3. 本 crate 不做迁移器注册表（当前只有 v1）；未来版本需补迁移函数。

## 6. 验收

- `pnpm provider-config-file:fixtures` →
  `crates/provider-config-file-codec/fixtures/files.json`
  - 成功：当前 schema 完整/最小、旧可编辑手动规则、旧完整手动规则；
  - 失败：缺失/负数/小数/字符串/未来/0 版本、未知顶层/ config 字段、缺 model rules、
    空 providerOrder 项、旧规则未知键、非法 reasoningLevel map。
- 成功用例比较 decode→encode 的完整 JSON；版本错误比较 message/version。
- `cargo test -p provider-config-file-codec`（2 条 Rust 单测 + 1 条 TS 回放）。
- `cargo clippy -p provider-config-file-codec --all-targets -- -D warnings`。
