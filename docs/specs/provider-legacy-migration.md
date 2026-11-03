# Spec：旧 Provider store 迁移（Rust `provider-legacy-migration`）

状态：2026-11-02 第四版（旧列表解析 + 迁移 + v2 helper + 旧 `config.json` 读取 +
Personal 层导入）。真源：`legacyModelProviderSerialized.ts`、
`legacyModeConfigProviderReader.ts`、`legacyPersonalProviderConfigImporter.ts`。

## 1. 范围

| Rust 入口                                      | TS 对应                                        |
| ---------------------------------------------- | ---------------------------------------------- |
| `parse_legacy_model_provider_list`             | `legacyModelProviderListSchema.parse`          |
| `migrate_legacy_model_provider`                | `migrateLegacyModelProviderConfig`             |
| `migrate_legacy_model_provider_list`           | 解析 + 逐条迁移                                 |
| `read_mode_config_providers`                   | `readLegacyModeConfigProviders`（纯转换部分）   |
| `read_mode_config_providers_from_str`          | 同上，额外解析 JSON 文本                        |
| `read_legacy_mode_config_providers`            | `readRawModeConfigFile` + `readModeConfigProviders` 的文件边界 |
| `read_legacy_mode_config_providers_with`       | 同上，文件读取可注入                             |
| `import_legacy_personal_provider_config`       | `importLegacyPersonalProviderConfig`           |
| `normalize_model_provider_base_url_for_kind`   | `normalizeModelProviderBaseUrlForKind`         |
| `normalize_model_provider_configured_base_url` | `normalizeModelProviderConfiguredBaseUrl`      |
| `default_supported_formats_from_endpoints`     | `getDefaultModelSupportedFormatsFromEndpoints` |
| `default_supported_formats_from_api_format`    | `getDefaultModelSupportedFormatsFromApiFormat` |
| `resolve_model_provider_api_format`            | `resolveModelProviderApiFormat`                |
| `resolve_model_provider_default_kind`          | `resolveModelProviderDefaultKind`              |
| `resolve_model_provider_kind_api_format`       | `resolveModelProviderKindApiFormat`            |
| `resolve_model_provider_runtime_base_url`      | `resolveModelProviderRuntimeBaseUrl`           |
| `resolve_model_provider_context_window`        | `resolveModelProviderContextWindow`            |
| `default_endpoint_path`                        | `getDefaultModelProviderEndpointPathForKind`   |
| `create_model_provider_model_config`           | `createModelProviderModelConfig`               |
| `get_model_provider_model_ids`                 | `getModelProviderModelIds`                     |
| `strip_legacy_claude_provider_mappings`        | `stripLegacyClaudeProviderMappings`            |
| `strip_model_provider_reasoning_patches`       | `stripModelProviderReasoningPatches`           |

`ProviderShape` / `EndpointsInput` 是 v2 helper 的输入投影（`apiFormat`、`defaultKind`、
`endpoints.baseURL/paths`），调用方从 store 记录构造；helper 本身不认识落盘格式。

输出是保序 `provider_config::JsonValue`：字段省略语义与 `JSON.stringify` 一致，
`undefined` 不落盘；整数 JSON 数字输出 `128` 而非 `128.0`。未知键在解析阶段忽略
（zod 非 strict 对象）。

### 1.1 旧 `config.json` 读取

`LegacyConfigReaderEnv` 注入 `runtime_env`、`bigmodel_api_origin` 与 `now_ms`，避免读取
进程环境或系统时钟。读取顺序：

1. `config.provider` 逐项把 OpenCode 结构转换为 v2 Provider：新运行态字段
   `kind` / `options.baseURL` / `api` / `npm` 优先；缺失时回退旧 `endpoints` /
   `apiFormat` / `defaultKind` / `mode` 扩展。`options.apiKey`、`options.headers`、
   模型 `limit` / `options.max_tokens` 与 `reasoning` variants 一并读取。
2. ZAI/BigModel 这组预置 Provider 的运行协议按 baseURL 路径纠正：BigModel Coding Plan
   的历史 `/coding/paas/v4` 回到 Anthropic；`/api/anthropic`、`/mode-plan/anthropic`
   为 Anthropic，其它 `/coding/paas/v4` 为 OpenAI-compatible。
3. BigModel Coding Plan 的 Anthropic baseURL 按环境归一：production 固定
   `https://open.bigmodel.cn/api/anthropic`；非 production 使用注入的
   `bigmodel_api_origin`，且生产域名会被替换为测试 origin。
4. `mode.deletedModels` 生成 tombstone 参与 store 归一化；最终读取结果过滤所有
   `deleted: true` 模型。
5. store 迁移：删除 `builtin:zapi` 与空 API Key 的 `default-*`；端点收敛为当前
   `defaultKind` 的 `baseURL + paths`；`apiFormat`/`defaultKind`/模型 metadata 归一；
   删除 `modelDisplayNames` / `modelSupportedFormats`；`providerMappings` 去 `claude`
   与 `mode*` 前缀。

### 1.2 旧 Effective Store → Personal 层

`import_legacy_personal_provider_config` 只提取仍能确认的用户意图，输出
`{ providers, models }` 的 JSON 投影（`providers` 对齐 `ProviderConfigMap.toJSON()`，
`models` 对齐 `ModelConfigRules.rules()` 的 `type/providerId/modelId/config`）：

1. `builtin:bigmodel` / `builtin:zai` 读成 `bigmodel-api` / `zai-api` 模板规则，只保留
   非空 API Key；其他 `builtin:*` 与 `account:*` 跳过。
2. 非内置 Provider 若声明了 `source` 且不是 `custom` 则跳过；`source` 缺失按旧数据
   处理。Provider 名去空白后与 ID 相同则不写 `providerName`。
3. Provider 规则保留 `enabled`；`group` 固定 `standard-personal`；`access` 始终为
   `api-key`，空 Key 省略，`apiKeyManagementUrl` 原样保留。
4. `api.type` 由 `apiFormat`/`defaultKind`/端点推导；`baseUrl` 先取 runtime
   `baseURL`，为空时回退 `normalizeModelProviderConfiguredBaseUrl(endpoints.baseURL)`。
5. 模型成员按旧顺序去重（区分大小写），跳过空 ID 与 `deleted: true`；成员列表非空时
   同时写 `personalModelIds` 与 `modelOrder`。仅正整数 `contextWindow` 生成
   `provider-model` 精确规则；重复 Provider/Model 原地替换，不追加重复项。



## 2. 旧记录校验

- 必填：`id`、`name`、`endpoints`、`apiKey`、`createdAt`、`updatedAt`；
- `endpoints` 的三段 `anthropic/openai/gemini` 缺省为 `""`，存在时必须是字符串；
- `models` 缺省 `[]`，必须为字符串数组；
- `apiFormat`、`source`、`systemDisabledReason` 走枚举校验；
- `headers`/`modelDisplayNames` 是 `string → string` 记录；
- `modelSupportedFormats` 是 `string → (anthropic|openai|responses|gemini)[]`；
- `providerMappings` 为对象；`claude` 子对象存在时四个槽位（haiku/sonnet/opus/reasoning）
  必须都是字符串，其余键原样保留；
- 任一条不合法即整体解析失败。

## 3. 迁移语义

1. `defaultKind`：`apiFormat` 优先映射；否则 Anthropic 端点非空 → `anthropic`，
   OpenAI 端点非空 → `openai-compatible`，都没有 → `anthropic`。
2. 旧端点 → v2 endpoints：按 kind 归一化（去空白、折叠重复绝对 URL、按 kind 去掉
   `/v1/messages`、`/messages`、`/responses`、`/chat/completions` 后缀、去尾斜杠）；
   同 origin 时输出 `baseURL` + 各 kind 的相对 `paths`，否则只输出完整 `paths`。
   OpenAI 端点按 `defaultKind === "openai"` 记为 `openai`，否则 `openai-compatible`。
3. 每个模型：`modelSupportedFormats[id]` 优先，否则按 `apiFormat`，再否则按旧端点
   推断；`gemini` 无对应 kind，全部格式都不可用时写
   `disabledReason: "legacy gemini format is not supported by mode.model-providers.v2"`。
4. 模型条目：`id`/`name` 去空白（空 name 省略）、`kinds` 去重、`defaultKind` 取
   defaultKind（若可用）或首个 kind、`modalities` 固定 `["text"]`、`contextWindow`
   固定 200000。
5. 输出字段顺序与 TS 返回对象一致；`providerMappings` 删除 `claude` 后保留其它键
   （空对象也保留）。
6. 旧的 `modelDisplayNames` / `modelSupportedFormats` 只参与迁移，不再出现在输出中。

## 4. 已知差异

1. **错误文案**：zod 的 issue 列表不复刻；Rust 返回单条错误字符串。
2. **非 http(s) scheme 的 origin**：TS `URL.origin` 对自定义 scheme 返回 `"null"`，
   Rust `url` 的 opaque origin 同为 `"null"`；合法 http(s) 输入行为一致，夹具未覆盖
   自定义 scheme 的共享 origin 形态。
3. 迁移只产出配置对象，不做文件 IO；落盘/合并仍由 provider-node 运行时负责。
4. `read_mode_config_providers` 只接收已解析的 config JSON；文件级入口自 2026-11-03 起
   由 `read_legacy_mode_config_providers` 提供（下面 §4.1）。文件读取端目前校验顶层与
   Provider 已知字段形状，完整的 zod 嵌套模型字段/枚举成员校验仍待补齐。

### 4.1 旧 `config.json` 文件读取边界

```text
read_legacy_mode_config_providers(path, env, read?)
  ├─ 读取失败：NotFound → None；其它 → stage=io path=<path> code=<code>
  ├─ JSON 解析失败 → stage=json path=<path>（不回显输入值）
  ├─ 顶层/provider/provider 条目形状校验 → stage=schema field=<path>
  ├─ 无 provider 键 → None
  └─ 有 provider → read_mode_config_providers → Some(list)
```

- IO 错误码：`PermissionDenied → EACCES`；其它优先用 `raw_os_error` 数字，缺省用
  `ErrorKind` 的调试名（与 Node 字符串错误码存在差异）。
- schema 校验覆盖：根对象、`$schema` 字符串、`provider` 对象、Provider 的
  `api/name/id/npm/kind/...` 字符串字段、`enabled/apiKeyRequired` 布尔、
  `createdAt/updatedAt` 数字、`env/whitelist/blacklist` 字符串数组、
  `options/providerMappings/endpoints` 对象、`headers` 字符串字典、`models` 对象，
  以及 `mode` 的 `enabled/headers/deletedModels` 形状。
- 未覆盖：模型条目的嵌套字段类型、枚举成员（kind/apiFormat/source 等）、端点的
  内部结构；这些仍以 `read_mode_config_providers` 的归一化结果为准，后续补齐。

## 5. 验收

- `pnpm provider-legacy:fixtures` → `crates/provider-legacy-migration/fixtures/legacy-providers.json`
  - `cases`（24 条）：最小 Anthropic、OpenAI 两种格式、双端点共享/不同 origin、
    重复 URL 折叠、大写 scheme、非法 URL、gemini-only、格式覆盖与去重、显示名 trim、
    可选字段、providerMappings 清理、缺省 models、空字符串，以及 6 类非法输入；
  - `helpers`：`apiFormat` / `defaultKind` / `runtimeBaseUrl`（含 kind 覆盖）/
    `configuredBaseUrl` / `modelIds` / `createModelConfig`（含 contextWindow 边界、
    0 maxOutputTokens、modalities 去重、全部可选字段）/ `stripReasoning` /
    `stripClaude` / `endpointPaths` / `kindApiFormat` / `defaultFormatsApiFormat` /
    `defaultFormatsEndpoints`。
- `pnpm provider-legacy-reader:fixtures` →
  `crates/provider-legacy-migration/fixtures/legacy-config-reader.json`
  - 10 条：新运行态字段覆盖旧扩展、旧 `mode` 回退（含 reasoningSpec/mode.reasoning/
    variants）、仅旧 endpoints、BigModel Coding Plan 生产/测试环境、ZAI/BigModel
    runtime 路径纠偏、store 清理、OpenCode `kind` + 规范 endpoints、缺省/空 provider。
- `pnpm provider-legacy-import:fixtures` →
  `crates/provider-legacy-migration/fixtures/legacy-personal-import.json`
  - 9 条：空输入、内置模板与跳过身份、自定义 Provider 成员/去重/上下文过滤、source
    过滤、空 Provider ID、重复规则原地替换、baseUrl 回退、空密钥/空 headers、
    空白 baseURL。
- `cargo test -p provider-legacy-migration`（24 条迁移逐项深比较 + 全部 helper 对拍 +
  10 条旧 config.json 读取对拍 + 9 条 Personal 导入对拍 + URL 折叠单测 +
  7 条文件 IO/JSON/schema 边界用例）。
- `cargo clippy -p provider-legacy-migration --all-targets -- -D warnings`。
