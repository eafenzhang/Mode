# Provider 纯配置模型（Rust）

> 范围：`@mode/provider` 的 **纯配置数据/序列化层** 的 Rust 移植。
> 真源（clean-room，只读本仓库 TS）：
> `packages/provider/src/config/{ids,provider-data-schema,rule-data-schema,manual-model-config,model-config,provider-config,schema,schema-validation}.ts`、
> `packages/provider/src/sources.ts`、`packages/provider/src/account-provider-state.ts`、
> `packages/shared/src/model-config.ts`、`packages/shared/src/config-schema.ts`、
> `packages/model-option-map/src/{tokenizer,parser}.ts`。
>
> Rust crate：`crates/provider-config`；对拍脚本：`scripts/generate-provider-config-fixtures.ts`
> （`pnpm provider-config:fixtures`）。

## 1. 范围与非范围

### 本轮做

- `ProviderConfig`、`ProviderApiConfig`、`ApiKeyAccessConfig`、`ZhipuAccountAccessConfig`、
  `ProviderLogo`、`ProviderTemplate`、`ProviderConfigMap`、`ProviderTemplateMap` 的解析/校验/序列化。
- `ModelConfig`、`ModelPropertiesConfig`、`ModelInputFormatConfig`、`ModelOutputFormatConfig`、
  `ModelOptionSpecsConfig`、`EnumOptionSpecConfig`、`LimitOptionSpecConfig` 的解析/校验/序列化。
- `ManualModelConfig`（手动配置只冻结产品开放的叶子）。
- 规则容器：`ModelConfigRules`（六类模型规则）、内置/个人 Provider 规则容器。
- 对应 zod schema 的接受/拒绝语义：严格对象（未知字段拒绝）、必填/可空/可选、枚举、URL、
  正整数安全整数、非空字符串、正则模式、Restricted CEL option map。
- `validateComplete`（`providerConfigDataSchema` / `modelConfigDataSchema` 的 complete 变体）
  的字段校验与问题码翻译（`required-field-missing` / `invalid-url` /
  `invalid-option-spec` / `invalid-config`）。
- `AccountProviderState(s)` 的纯形状类型（下一步 Account overlay 的输入）。

### 本轮不做（后续切片）

- config-service 的文件轮询、revision 计算、Built-in/Personal/Account 合成。
- registry / resolver / model listing / remote catalog / provider provisioning。
- `ProviderConfigMap.overlay`、`ModelConfigRules.resolve`、`resolveOwnedOrder` 的排序编排。
- AccountProviderService 与 `createAccountProviderConfigSnapshot` 的 revision 生成。
- 任何文件 IO、网络、凭据、进程或 UI 依赖。

## 2. 数据模型与字段

### 2.1 Provider

| 类型 | 字段（序列化顺序即此顺序） | 可空/可选 |
| --- | --- | --- |
| `ApiKeyAccessConfig` | `type`, `apiKey`, `apiKeyManagementUrl` | 后两个 nullable optional |
| `ZhipuAccountAccessConfig` | `type`, `accountType`, `mode`, `entitled` | 后三个 nullable optional |
| `ProviderApiConfig` | `type`, `baseUrl`, `headers` | 全部 nullable optional |
| `ProviderLogo` | `type`（恒 `"builtin"`）, `key` | `key` 必填非空 |
| `ProviderConfig` | `group`, `logo`, `access`, `api`, `builtinModelIds`, `personalModelIds`, `modelOrder`, `visibility` | 全部 nullable optional |
| `ProviderTemplate` | `templateId`, `templateNameMap`, `config` | 无 |
| `ProviderTemplateNameMap` | `zh-CN`, `en-US` | 均可选；固定按该顺序输出 |

`ProviderConfigMap.toJSON()` 是数组，元素为
`{ providerId, templateId?, providerName?, enabled?, config }`（顺序固定）。
`ProviderTemplateMap.toJSON()` 是 `ProviderTemplate[]`，按插入顺序。

### 2.2 Model

| 类型 | 字段（序列化顺序） | 可空/可选 |
| --- | --- | --- |
| `ModelInputFormatConfig` | `supportsText`, `supportsImage`, `supportsVideo`, `supportsAudio`, `supportsPdf` | 全部 nullable optional |
| `ModelOutputFormatConfig` | `supportsText` | nullable optional |
| `ModelPropertiesConfig` | `requiresMfjsToolSchema`, `contextWindow`, `inputFormat`, `outputFormat`, `supportsToolCall`, `supportsJsonSchemaOutput`, `supportsNativeWebSearch`, `supportsMidConversationSystem` | 全部 nullable optional |
| `EnumOptionSpecConfig` | `values`, `map` | nullable optional |
| `LimitOptionSpecConfig` | `max`, `map` | nullable optional |
| `ModelOptionSpecsConfig` | `reasoningLevel`, `maxOutputTokens` | nullable optional |
| `ModelConfig` | `enabled`, `properties`, `optionSpecs` | 全部 nullable optional |

### 2.3 ManualModelConfig

`manualModelConfigSchema` 不是 full config 的稀疏版本，而是明确的叶子白名单：

- `enabled`: nullable optional boolean
- `properties`: **必填且非 null**，严格对象：
  - `contextWindow`（必填，正整数）
  - `supportsJsonSchemaOutput` / `supportsNativeWebSearch` / `supportsMidConversationSystem`（必填 boolean）
  - `inputFormat`（必填对象）：`supportsImage` / `supportsVideo` / `supportsPdf`（必填 boolean）
- `optionSpecs`: **必填且非 null**，严格对象：
  - `reasoningLevel`（必填完整 enum spec：`values` + `map`）
  - `maxOutputTokens`（必填对象）：`max`（必填正整数）——不允许 `map`

未知字段一律拒绝（包括 `properties.supportsText`、`maxOutputTokens.map` 等）。

### 2.4 无默认值

解析层不补默认值：缺失字段保持“缺失”，不会被写成 `false`/`0`/空数组。
唯一的隐式默认是 `ApiKeyAccessConfig` 构造函数的 `type ?? "api-key"`，但所有 schema
都要求 `type`，因此解析路径上不可达。

## 3. 校验规则（与 zod 对齐）

通用：

- 对象一律 **strict**：出现未声明键时拒绝，问题路径是对象自身路径（zod `unrecognized_keys`）。
- `nullable optional`：缺省合法；`null` 合法并保留为 `null`；非 null 时按内层 schema 校验。
- `optional`（非 nullable）：缺省合法；`null` 非法。
- `string().min(1)`：空字符串拒绝。
- `number().int().positive()`：必须是 **安全整数**（`Number.isInteger` + `Number.isSafeInteger`
  语义），且 `> 0`；`0`、负数、小数、`1e20`、`1e400` 全部拒绝。
- 非空字符串 refine：`value.trim().length > 0`（纯空白拒绝）。
- URL：`z.string().url()`（`new URL()` 语义的近似实现，见 §5）。
- 正则模式：先 `min(1)`，再要求 `new RegExp("^(?:"+pattern+")$")` 可编译。
- Restricted CEL：`compileModelOptionMap(source, variableName)`，只做编译期可接受性
  （tokenize + parse + “顶层/条件分支必须返回对象”），不求值。

Access：

- `api-key` / `zhipu-coding-plan-api-key`：`type` 枚举；`apiKey`、`apiKeyManagementUrl` 可空可选，
  后者必须是 URL。
- `zhipu-account`：`type` 字面量；`accountType ∈ {zai,bigmodel}`、`mode ∈
  {start-plan,individual-coding-plan,team-coding-plan,off-peak}`、`entitled:boolean`，均可空可选。
- 判别联合按 `type` 分派；`type` 缺失/未知时在 `type` 路径报错。

Provider API：

- 稀疏版：`type ∈ {anthropic-messages,openai-chat-completions,openai-responses}`、
  `baseUrl` URL、`headers: Record<string,string>`，全部可空可选。
- 个人版：`baseUrl` **不做 URL 校验**（允许编辑中的半成品），其余同稀疏版。
- 完整版：`type`、`baseUrl`（非空 + URL）必填；`headers` 可空可选。

Provider 变体：

| 变体 | 额外规则 |
| --- | --- |
| 通用 `providerConfigRuleSchema` | `providerId` 非空；`templateId`/`providerName` 可空可选；`enabled` 可选 |
| 内置 Provider | `config` 禁止 `personalModelIds`/`modelOrder`；`group` 必填且不含 `standard-personal` |
| 个人 Provider | `config` 禁止 `builtinModelIds`；`group` 只允许 `standard-personal`（可空）；`api.baseUrl` 不校验 URL；`providerId` 以 `account:` 开头时 `config.access` 必须缺省（含 `null` 也拒绝） |
| 模板 | `config` 只允许 `logo`/`access`/`api`/`builtinModelIds`；`access` 只允许两种 api-key 类型且禁止 `apiKey` 字段；`templateNameMap` 只允许 `zh-CN`/`en-US` |
| Account | 每项 `config` 只允许 `builtinModelIds` 与 `{type:"zhipu-account", entitled?}`；`access` 内 `accountType`/`mode` 等未知键拒绝 |

规则容器：

- `builtinProviderConfigRulesSchema`：`templateRules`、`providerRules` 必填数组；重复
  `templateId` / `providerId` 在对应索引路径报错。
- `personalProviderConfigRulesSchema`：`providerRules` 必填；重复 `providerId` 报错。
- `builtinModelConfigRulesSchema`：五个数组全部必填。
- `personalModelConfigRulesSchema`：`providerModelRules`、`manualProviderModelRules` 必填；
  同一 `(providerId, modelId)` 不能同时出现在智能规则与手动规则中。

## 4. 序列化顺序

TS 的 `toJSON()` 与 `JSON.stringify` 属性顺序是协议 payload 的一部分。Rust 侧：

- 普通对象按 §2 表中顺序生成（不依赖 serde_json 的 `Map`）。
- `Map`/record 类（`headers`、`ProviderConfigMap`、`ProviderTemplateMap`）用 `indexmap::IndexMap`
  保留插入顺序。
- `Field::Absent` 不输出；`Field::Null` 输出 `null`；`Field::Value` 输出值。
- `ModelConfigRules.toJSON()` 的分组顺序：
  `modelRules`, `modelApiRules`, `providerSiteRules`, `templateModelRules`,
  `builtinProviderModelRules`, `manualProviderModelRules`。
  - 前五组按 schema 重新排序：`model` 为 `modelMatch,config`；`model-api` 为
    `modelMatch,config,apiTypeMatch`；`provider-site` 为
    `modelMatch,config,baseUrlMatch,apiTypeMatch?`；`template-model` 为
    `templateId,modelId,config`；内置精确规则为 `modelId,config,providerId`。
  - `manualProviderModelRules` 来自 `#collect`，属性顺序是 `modelId,providerId,config`
    （`config` 被 `{...identity, config}` 移到最后）。
- `toPersonalJSON()` 两组都是 `modelId,config,providerId`。
- 字符串转义与 `JSON.stringify` 一致（compact，不转义非 ASCII / `/`）。

对拍用 `output`（原始字符串）做逐字节比较，另用 `outputJson` 做与顺序无关的深层比较。

## 5. 与 TS 的已知差异与风险

1. **URL 校验**：`z.string().url()` 委托 JS `URL` 构造器；Rust 侧是保守近似：
   校验 scheme、特殊 scheme 的非空 host、端口范围与 authority 非法字符。查询/路径中的空格
   （JS 会百分号编码）被接受；极端 URL 方言可能与 JS 不一致。fixtures 覆盖常见合法/非法形态，
   未覆盖的差异按“Rust 更严格/等价”记录。
2. **JS 正则方言**：Rust 只做语法可接受性检查（分组/字符类配对、重复量词、命名组重复、
   `(?...)` 前缀、基础量词范围），不实现完整 V8 正则文法；`\p{...}`、复杂字符类范围、
   内联修饰符等未覆盖。fixtures 只用常见模式。
3. **Restricted CEL**：完整移植 tokenizer/parser 与“返回对象”检查；只验证不执行。
   JS `\s` 与 Rust `is_whitespace` 在少数 Unicode 空白字符上可能有边缘差异。
4. **错误文案**：接受/拒绝与错误路径对齐；zod 的 message/code 不做逐字复刻
   （`validateComplete` 的翻译码除外）。
5. **未实现 overlay / resolve / revision**：这些属于 config-service 与 Account overlay 切片。

## 6. 下一片

1. `config-service`：Built-in/Personal/Account 三层读取、revision 计算与变更通知。
2. `ProviderConfigMap.overlay` 与三态字段覆盖语义（含 access 类型切换）。
3. `ModelConfigRules.resolve`（正则匹配、URL 归一、手动规则清空系统叶子）。
4. Account overlay：`createAccountProviderConfigSnapshot` / fail-closed overlay /
   `AccountProviderStates` 的 revision 输入。
5. `resolveOwnedOrder` 与 `modelOrder` 持久化编排。
