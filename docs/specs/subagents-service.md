# Spec：Subagent 管理服务（Rust `subagents-service`）

状态：2026-10-31 首版。对应 `packages/services/src/subagents/` 的
`ISubagentsService`（8 个方法）与 Host 启动前的一次性 `prepareRuntimeState`。

## 1. 范围

| Rust                                | 真源                            | 说明                                          |
| ----------------------------------- | ------------------------------- | --------------------------------------------- |
| `list`                              | `subagentsService.ts` `list`    | 内置 + 用户/工作区 Markdown + 插件 agent 合并 |
| `set_enabled`                       | `setEnabled`                    | 写 `agents-state.json` 的 `disabledAgentIds`  |
| `set_built_in_model_override`       | `setBuiltInModelOverride`       | 内置 agent 的模型选择覆盖                     |
| `set_plugin_agent_model_override`   | `setPluginAgentModelOverride`   | 插件 agent 的模型覆盖（按稳定 id）            |
| `get_primary_user_agents_directory` | `getPrimaryUserAgentsDirectory` | 返回并创建用户级 agents 根                    |
| `create_agent`                      | `createAgent`                   | 校验后写 `<root>/<name>.md`（`wx`）           |
| `update_agent`                      | `updateAgent`                   | 覆盖写、迁移禁用记录、删除旧文件              |
| `delete_agent`                      | `deleteAgent`                   | 删文件并清理禁用记录                          |
| `prepare_runtime_state`             | `prepareRuntimeState`           | 迁移旧 markdown 与旧 state 字段（非 RPC）     |

不在本 spec 内：频道注册开关（壳侧 `MODE_HOST_INPROCESS_CHANNELS`；适配器
`crates/host-channels/src/subagents.rs` 已就位，默认不接管）、Agent 运行时对
subagent 的加载。

## 2. 路径与根

`home_dir` 由调用方传入；`from_process()` 复刻 TS 的
`HOME.trim() || USERPROFILE.trim() || homedir()`。

- 存储根：`<home>/.mode/cli/config.json` 的 `storage.dir`（trim 非空）或 `~/.mode`；
  `~/` 前缀展开，相对路径按当前工作目录 `resolve()`。
- 用户 agents 根：`<storage>/agents`。
- 工作区 agents 根：`<workspace>/.mode/agents`。
- 状态文件：`<storage>/v2/agents-state.json`。
- 插件存储根：`<storage>/cli/plugins`（存储根 basename 已是 `cli` 时不重复追加）。

## 3. 状态文件

```json
{
  "builtInModelSelectionOverrides": { "general-purpose"?: Selection, "Explore"?: Selection },
  "pluginAgentModelSelectionOverrides": { "plugin:<pluginId>:<bare>"?: Selection },
  "disabledAgentIds": ["user:workspace:alpha"]
}
```

- 读：缺失/非对象/非法 JSON 一律视为空状态；只接受两个内置键与 `plugin:` 前缀键的
  **严格** `ModelSelection`；`disabledAgentIds` 只收非空字符串（保留原文，不 trim）。
- 未知键与旧字段在写回时原样保留（`other` 映射）。
- `setEnabled`：`enabled=false` 追加 id 后排序；`true` 移除。
- 覆盖写入：`selection` 存在则写入，缺失则删除该键。
- 落盘：`JSON.stringify(next, null, 2)` 同形（两空格、无尾随换行），原子写。

## 4. Markdown 解析（`parseSubagentMarkdown`）

1. 去 BOM、`\r\n` → `\n`；首行 `trim() === "---"`，闭合行为第一个 `trim() === "---"` 的行。
2. 宽松 frontmatter（`subagentMarkdown.ts` 的 `parseLooseFrontmatter`）逐行覆盖 YAML 值：
   - 行首 `- ` 追加到上一个空值键的数组；`key: value` 只接受
     `^[A-Za-z][A-Za-z0-9_-]*\s*:\s*` 形式的键。
   - 标量：`true`/`false`、`^\d+$` → 数字、`[...]` 顶层逗号拆分、`{...}` 尝试 `JSON.parse`，
     其余去一层引号；` #` 起视为行内注释（引号内除外）。
3. YAML 采用受支持子集（嵌套映射/序列、块标量 `|`/`>`、行内流式集合、单双引号标量）；
   子集外输入整体判失败，等价于 TS `yaml.parse` 抛错后的回退；`mcpServers` 只取 YAML 数组。
4. 字段：`name`/`description` 必填（trim 非空，`description` 中字面 `\n` 还原为换行）；
   `color`（8 色）与 `permissionMode`（auto/plan）枚举；`maxTurns` 正整数；
   `tools`/`disallowedTools` 支持逗号+空白拆分（括号内不拆）；`skills` 逗号/空白拆分；
   `background`/`injectAgentsMd` 接受布尔或 `"true"/"false"`（忽略大小写）。
5. `id = <source>:<scope>:<name.trim().toLowerCase()>`；`source` 为 `built-in` 或 `user`；
   `readOnly = scope === "built-in"`；`enabled = true`（列表阶段按状态覆盖）。
6. 诊断：`agent_missing_frontmatter`、`agent_missing_required_frontmatter`、
   `agent_read_failed`、`agent_ambiguous_name`。

## 5. 序列化（`serializeSubagentMarkdown`）

`name`/`description` 始终双引号并转义 `\ " \n \r`；`color`、`model`/`thoughtLevel`、
`permissionMode` 用「安全 plain 标量否则双引号」规则；`tools`/`disallowedTools`/`skills`
写块序列；`maxTurns` 数字、`background`/`injectAgentsMd` 布尔；`mcpServers` 全字符串时
写块序列，否则按 YAML 块风格写出对象数组。正文 `trim` 非空时前后各一换行，否则仅 `\n`。

## 6. 模型选择

- `model` + `thoughtLevel` 是 Markdown 的正式字段；`inherit`/`main`/`sonnet`/`opus`/`haiku`
  视为未指定。
- `custom:` 前缀按 `custom-model-value.ts` 编解码（`encodeURIComponent` 语义，
  兼容旧 `custom:builtin:<provider>:<model>` 形态）；普通值按 `provider/model$level` 解析。
- `formatSubagentMarkdownModel`：provider 以 `custom:` 开头、含 `/`，或 model 含 `$` 时用
  custom 编码，否则 `provider/model`。
- 旧 ID 迁移（只用于已发布数据）：`builtin:zai|bigmodel` → `zai-api|bigmodel-api`，
  `builtin:*-start-plan|-coding-plan` → `account:*`；官方 GLM 型号按大小写不敏感归一。

## 7. 发现与合并

1. 内置：`general-purpose`（blue、`injectAgentsMd`、`tools:["*"]`）与 `Explore`
   （cyan、只读工具集）；`modelSelection` 与 `modelSelectionOverride` 同取状态覆盖。
2. 文件 agent：用户根（`userScopeAvailable` 时）与工作区根（`allRuntimeScopes` 时）；
   `.md`/`.markdown` 递归收集（不跟随软链目录），按路径 `localeCompare` 升序。
3. 插件 agent：`enabledPlugins` 只认 `true`；官方缓存按名字+版本目录（数字感知降序，
   跳过 `.backup`/`.seed-lock`/`.tmp-`）补缺失身份；`suppressedBuiltins` 优先。
   清单 `agents` 字段（字符串/数组）追加到默认 `agents/` 根，越界路径丢弃。
   id 为 `plugin:<pluginId>:<bare 小写>`；显示名为 `<插件名>:<bare>`；只读。
   裸名唯一且不与内置/文件 agent 重名时追加运行时别名 `plugin:<id>:alias:<bare>`。
4. 排序：用户/工作区 agent 与插件 profiles 均按 `name` 升序；运行时列表按插入序
   「同名后到覆盖、位置保留」去重。
5. `settingsUserOnly`：只返回内置 + 用户 agent；插件 profiles 仍在 `pluginAgents` 字段。
6. 能力位：`userScopeAvailable` 由 `isDesktopRuntime ?? Boolean(MODE_PROCESS_LABEL)` 决定；
   false 时 `userScopeReason: "desktop_only"` 且不扫描用户根。
7. 列表阶段先把用户根 Markdown 的旧 Provider 迁移一次；失败进诊断
   `agent_read_failed`（消息 `Subagent Markdown migration failed; original file preserved`）。

## 8. 写操作

- `create`：名称 3–50 字符、`^[a-zA-Z0-9-]+$`、描述与系统提示非空；保留名
  `general-purpose`/`Explore` 拒绝；目标 `<root>/<name.trim().toLowerCase()>.md`
  已存在时报 `Agent file "<文件名>" already exists`；先序列化并回读校验，再以
  `create_new` 写入。
- `update`：同样的校验；`oldFilePath` 与目标不同且目标已存在时报同样错误；
  覆盖写后迁移 `disabledAgentIds`（旧 id → 新 id），再删除旧文件。
- `delete`：删除文件并移除该 id 的禁用记录。
- 所有写操作在同一服务实例内串行（互斥锁）。

## 9. 迁移

- `agents-state.json`：读前一次性导入旧双 map（`builtInModelOverrides` /
  `builtInThoughtLevelOverrides` / `pluginAgentModelOverrides` /
  `pluginAgentThoughtLevelOverrides`）；新字段存在即为权威，不再解释旧值。
  写回采用「解析后语义比较」：内容不变不重写；损坏文件保留原文。
- 用户 agents 根 Markdown：只重写 frontmatter 中 `model:` 单行的旧 Provider/型号，
  保留引号风格与无关格式；写前比较原文，避免覆盖并发修改；失败保留原文件。

## 10. 已知差异（不与 TS 逐字节对齐）

1. **状态文件键序**：TS 保留插入序，Rust 按 JSON map 的字母序写出；对拍时键排序后比较。
2. **YAML 对象键序**：`mcpServers` 对象数组写回时按键字母序（TS 保留输入序），语义一致。
3. **排序**：`localeCompare` 用「小写折叠 + 大小写次序」近似；ASCII 一致。
4. **Markdown 迁移**：不保留文件权限位与 inode 并发校验（改为内容比较）。
5. **错误文本**：文件系统错误的文案来自各自运行时（如删除不存在文件被忽略）。

## 11. 验收

- `pnpm subagents:fixtures` 生成 `crates/subagents-service/fixtures/subagents.json`
  （7 个用例：基础列表、CLI 能力位、settingsUserOnly、状态写、创建/更新/删除、
  插件 agent、旧数据迁移）。
- `cargo test -p subagents-service` 回放同一份文件树与操作序列，逐项深比较八个方法
  的结果与结束树（状态文件按键排序后比较）。
- `crates/host-channels/tests/subagents_channel.rs` 覆盖传输层参数、能力位与错误文本。
