# Provider 配置只读源（Rust）

> 范围：`@mode/provider-node` 的**只读文件层**与 `@mode/provider` 的
> **两层组合快照**的 Rust 移植，作为后续 `ProviderSource` 真实实现的基线。
>
> 真源（clean-room，只读本仓库 TS）：
> `packages/provider-node/src/provider-config-file-codec.ts`、
> `packages/provider-node/src/personal-provider-config-repository.ts`、
> `packages/provider-node/src/mode-builtin-release.ts`、
> `packages/provider-node/src/mode-builtin-provider-config-source.ts`、
> `packages/provider-node/src/mode-builtin-provider-config-materializer.ts`、
> `packages/provider-node/src/runtime-paths.ts`、
> `packages/provider/src/config-service.ts`、
> `packages/provider/src/sources.ts`。
>
> Rust crate：`crates/provider-config-source`（依赖 `crates/provider-config` 的纯模型）。
> 对拍：`scripts/generate-provider-config-source-fixtures.ts`
> （`pnpm provider-config-source:fixtures`）+ `crates/provider-config-source/tests/parity.rs`。

## 1. 范围与非范围

### 本轮做（只读）

- Personal 文件解码：`schemaVersion`、严格 `config` 信封、legacy manual 归一、
  `providerOrder` / `defaultModelSelection`。
- Personal 读取恢复：缺失、JSON 损坏、schema 不兼容或规则不合法时，进程内降级为
  **空 Personal 层**（与 `NodePersonalProviderConfigRepository.read()` 一致）。
- Builtin Release 解码/规范序列化、`builtin:zapi` 拒绝、Bundled/Active 候选选择。
- Active 缺失/损坏时回落到 Bundled；同 revision 不同内容时选择 Bundled。
- `mode-builtin:<releaseRevision>:<sourceKey>` 的 revision；`sourceKey = sha256(resolve(activePath))`。
- 两层组合 Snapshot 的字段与 `revision = JSON.stringify([builtinRevision, personalRevision])`。

### 本轮不做（后续切片）

- 轮询 / watch（`onDidChange`、`fs.watch`、polling 去重）。
- 写路径：`update` / `replace` / `migrate` / `importLegacy`、`atomicWrite*`、`withFileLock`。
- Active 缓存的物化写入（本切片只选择候选，不写 cache，也不建目录）。
- Remote Release 同步、LKG、Endpoint 隔离来源、registry/resolver、model listing。
- Account overlay / `createAccountProviderConfigSnapshot`。
- `MODE_*_PROVIDER_CONFIG_FILE` 环境变量解析（`runtime-paths.ts` 只作为路径来源说明）。

## 2. 文件与公开 API

```
crates/provider-config-source/
  Cargo.toml
  src/lib.rs        // 公开类型、错误、read_* 入口
  src/resolve.rs    // Node path.resolve 等价实现 + sourceKey
  src/personal.rs   // Personal 解码/归一/恢复
  src/builtin.rs    // Release 解码/序列化/候选选择
  tests/parity.rs   // 与 TS fixture 对拍
  tests/unit.rs     // 恢复、归一、sourceKey、组合 revision
  fixtures/provider-config-source.json
```

公开入口：

| 函数 | 语义 |
| --- | --- |
| `read_personal_layer(path) -> PersonalLayer` | 永远返回层；读取/解码失败降级为空层 |
| `decode_personal_document(&JsonValue) -> Result<PersonalLayer, SourceError>` | 严格解码，供测试与后续写入路径复用 |
| `read_mode_builtin_layer(bundled, active?) -> Result<BuiltinLayer, SourceError>` | 候选读取与选择；两者都不可用时返回错误 |
| `decode_mode_builtin_release(&JsonValue) -> Result<ModeBuiltinRelease, SourceError>` | Release 严格解码 |
| `serialize_mode_builtin_release(&ModeBuiltinRelease) -> String` | 与 TS `serializeModeBuiltinRelease` 一致的紧凑 JSON |
| `read_snapshot(bundled, personal, active?) -> Result<ProviderConfigSnapshot, SourceError>` | 两层组合 |
| `source_key_for_path(path)` / `source_key_with_cwd(path, cwd)` | `sha256(resolve(path))`，后者用于确定性测试 |

每个层/快照类型都有 `to_json()` 与 `to_json_string()`，字段顺序与 TS 快照字面量一致。

## 3. Personal 层

### 3.1 revision 与规范编码

`NodePersonalProviderConfigRepository.snapshotFromUpdate` 的 revision 不是原始文件字节的
哈希，而是**规范编码后的 UTF-8 JSON 文本**的 sha256：

```json
{
  "schemaVersion": 1,
  "config": {
    "providerOrder": [],
    "providerConfigRules": { "providerRules": [] },
    "modelConfigRules": {
      "providerModelRules": [],
      "manualProviderModelRules": []
    }
  }
}
```

- 空层必须带 `providerOrder: []`（`emptyUpdate()` 显式给出空数组）。
- `defaultModelSelection` 仅在定义时输出。
- revision = 上述紧凑字符串（`JSON.stringify` 口径）的 sha256 hex，小写。

因此同一语义的输入即使文件键顺序不同、含多余空白，revision 仍相同；读取时先解码再按
规范形状计算。

注意：TS `toPersonalJSON()` 会在编码边界用 `personalModelConfigRulesSchema` 重新解析，
manual 规则的 `config` 因此采用 **manual schema 的字段顺序**（`enabled, properties
{contextWindow, supportsJsonSchemaOutput, supportsNativeWebSearch,
supportsMidConversationSystem, inputFormat}, optionSpecs`），而不是 `ModelConfig.toJSON()`
顺序。`crates/provider-config` 的 `to_personal_json()` 只做结构投影；本 crate 在计算
revision 时用 `ManualModelConfig::to_json()` 单独复刻该重排，未修改 `provider-config`
语义。

### 3.2 解码接受范围

- 顶层严格对象：`{ schemaVersion, config }`，未知字段拒绝。
- `schemaVersion`：必须存在且是**非负整数**；`> 1` 报“高于当前支持版本”；`< 1`
  报“缺少迁移器”（当前无迁移器）；缺失/非整数/负数同样拒绝。
- `config` 严格对象：`providerOrder?`、`providerConfigRules`（必填，任意值）、
  `modelConfigRules`（必填，任意值）、`defaultModelSelection?`。
- `providerOrder`：存在时必须是字符串数组，每项长度 ≥ 1（不 trim）。
- `defaultModelSelection`：`modelSelectionSchema` 口径，`providerId` / `modelId` 与
  `options.reasoningLevel` 都会 trim 且要求非空；`options` 严格对象。
- `providerConfigRules` 交给 `provider_config::parse_personal_provider_config_map`。
- `modelConfigRules` 先做 legacy manual 归一，再交给
  `provider_config::parse_personal_model_config_rules`。

### 3.3 Legacy manual 归一

与 `normalizeLegacyManualRules` 一致，仅当 `manualProviderModelRules` 是数组时处理：

1. 规则不是对象，或 `manualModelConfigSchema` 能解析 `rule.config` → 原样保留。
2. 否则尝试 `z.union([legacyCompleteManualSchema, legacyEditableManualSchema])`：
   - **legacy complete**：完整 `completeModelConfigDataSchema` 形状，`enabled` 放宽为
     nullable/optional。
   - **legacy editable**：完整 manual 形状 + `properties.requiresMfjsToolSchema`
     必填 boolean。
3. 命中后按 `extractManualModelConfig` 的字段白名单提取：
   `enabled?`、`properties.{contextWindow, supportsJsonSchemaOutput,
   supportsNativeWebSearch, supportsMidConversationSystem, inputFormat.{supportsImage,
   supportsVideo, supportsPdf}}`、`optionSpecs.{reasoningLevel.{values,map},
   maxOutputTokens.{max}}`；丢弃 `requiresMfjsToolSchema`、`outputFormat`、
   `supportsToolCall`、`inputFormat.supportsText/supportsAudio`、`maxOutputTokens.map`。
4. 两者都不命中 → 原样保留，随后由严格 personal 规则解析拒绝。

### 3.4 恢复语义

`NodePersonalProviderConfigRepository.read()` 对**任何**读取/解析异常都返回空层，并且
不覆盖原文件。本 crate 的 `read_personal_layer` 照搬：

| 输入 | 结果 |
| --- | --- |
| 文件不存在（ENOENT） | 空层 |
| JSON 语法错误 | 空层 |
| 高低 schemaVersion / 缺 schemaVersion | 空层 |
| 信封未知字段、规则非法、重复身份 | 空层 |
| 其他 IO 错误（权限、目录等） | 空层 |

空层的 revision 固定为 §3.1 规范编码的 sha256。

## 4. Builtin 层

### 4.1 Release 解码

严格对象 `{ schemaVersion, revision, config }`：

- `schemaVersion` 必须为 `1`。
- `revision` 必须是 `0..=Number.MAX_SAFE_INTEGER` 的整数。
- `config` 严格对象，`providerConfigRules` 与 `modelConfigRules` 必填。
- Provider 规则经 `parse_mode_builtin_provider_config_rules`；若包含
  `builtin:zapi` 则整份 Release 拒绝。
- Model 规则经 `parse_mode_builtin_model_config_rules`。

规范序列化：

```json
{"schemaVersion":1,"revision":<n>,"config":{"providerConfigRules":{"templateRules":[...],"providerRules":[...]},"modelConfigRules":{...}}}
```

`modelConfigRules` 使用 `toModeBuiltinJSON()`（五组，不含 manual 组）。

### 4.2 候选选择

`activePath = active?.trim() || bundledPath`。读取时：

1. `bundled` 候选永远尝试读取；仅当 `activePath != bundledPath` 时读取 `active`。
2. 两者都有效且 revision 相同但规范序列化不同 → **选 Bundled**（Active 视为坏缓存）。
3. 否则在有效候选中取 revision 最大者；相同 revision 且内容相同时取 Bundled。
4. 没有有效候选 → 返回 `Unavailable` 错误（对应 `AggregateError`）。

缺失文件（ENOENT）视为“无候选”；其他读取/解码失败视为“无效候选”。

### 4.3 revision 与 sourceKey

```
sourceKey = sha256_hex(resolve(activePath))
revision  = "mode-builtin:" + release.revision + ":" + sourceKey
```

`resolve` 必须与 Node `path.resolve` 一致：

- Windows：`/` 归一为 `\`，折叠重复分隔符与 `.`/`..`，去掉尾部分隔符，
  保留盘符大小写；相对路径拼到当前工作目录。
- POSIX：按 `/` 归一。
- UNC / 盘符相对路径（`C:foo`）不在本切片保证范围内。

层 JSON 字段顺序：`revision`、`providers`、`providerTemplates`、`models`。

## 5. 组合 Snapshot

`ProviderConfigService.read()` 的字段与顺序：

```
revision                       = JSON.stringify([modeBuiltin.revision, personal.revision])
modeBuiltinRevision
personalRevision
modeBuiltinProviders
modeBuiltinProviderTemplates   // 缺省为空 TemplateMap
personalProviders
modeBuiltinModelRules
personalModels
personalProviderOrder          // 缺省为 []
```

`modeBuiltinModelRules` / `personalModels` 在 `JSON.stringify` 时走
`ModelConfigRules.toJSON()`（内置五组 + `manualProviderModelRules`），因此快照里的模型
规则始终包含空/非空 manual 组。

## 6. 与 TS 的已知差异

| 差异 | 说明 |
| --- | --- |
| 不做规范化写盘 | TS 在文件非规范时加锁重写；本切片只返回规范快照，不写文件。读结果相同，写失败导致的降级不复现。 |
| 不建目录 / 不写 Active | TS `read()` 会 `mkdir` 并在需要时物化 Active；本切片只读。目录不可创建导致的 Bundled-only 回落不复现。 |
| 无锁 | 不调用 `withFileLock`；依赖原子替换的可观察语义。 |
| 无轮询/watch | `onDidChange`、`#ensurePolling`、`fs.watch` 不在本切片。 |
| 无 remote/LKG | `applyRemoteRelease` 与 Endpoint 隔离来源不在本切片。 |
| UTF-8 解码 | Node `readFile(..., "utf8")` 用 U+FFFD 替换非法序列；本 crate 用 `from_utf8_lossy` 对齐。 |
| 空 Personal 路径 | TS 构造器拒绝；本 crate 的 `read_personal_layer` 按读取失败降级为空层。`read_snapshot` 沿用该行为。 |

## 7. 下一片

1. `ProviderSource` 抽象 + `onDidChange` + 轮询/revision 去重。
2. 写路径：`update`、`replace`、`importLegacy` 与规范写盘、文件锁。
3. Active 缓存物化与 Remote Release 同步（LKG、Endpoint 隔离）。
4. Account overlay / `createAccountProviderConfigSnapshot` 与 Provider 事实源接管。
5. registry/resolver/model listing 的 Rust 侧实现或桥接。
