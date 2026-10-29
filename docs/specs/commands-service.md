# Spec：commands 频道（`crates/commands-service`）

状态：2026-10-29 实现（W3 下一域，6 个方法）。行为真源（clean-room，只读本仓库 TS）：

- `packages/services/src/commands/commands.ts`（`ICommandsService`，6 个方法）
- `packages/services/src/commands/commandsService.ts`（目录来源优先级、读写、插件命令）
- `packages/services/src/commands/commandFileParser.ts`（markdown frontmatter 手写解析）
- `packages/services/src/plugins/installedPluginRoots.ts`（`installed_plugins.json`）
- `packages/shared/src/command-types.ts` / `settings-source.ts` / `mode-agent-policy.ts`
- `packages/shared/src/plugin-marketplaces.ts`（只移植命令发现的三个符号）
- `packages/shared/src/node/officialPluginCache.ts`（只移植 `resolveOfficialPluginCacheRoot`）
- `packages/shared/src/data-root.ts`（`MODE_DATA_ROOT_DIR_NAME = ".mode"`）

Rust 侧不拥有任何新状态：读的是用户已有目录与 `~/.mode/cli/config.json`，写回同一份
配置文件（沿用 TS 的字节形状：2 空格缩进 + 尾随换行）。

## 方法

| 方法 | 参数（camelCase） | 返回 |
| --- | --- | --- |
| `list` | `{agentSource?, workspacePath?, workspaceIdentity?}` | `CommandsListResult` |
| `writeCommandFile` | `CommandCreateParams` | `{command: UserCommand}` |
| `updateCommandFile` | `CommandUpdateParams` | `{command: UserCommand}` |
| `deleteCommandFile` | `CommandDeleteParams` | `undefined` |
| `setCommandEnabled` | `CommandSetEnabledParams` | `undefined` |
| `getPrimaryUserCommandsDirectory` | `{agentSource?}` | `{path}` |

`workspaceIdentity` 在 TS 里**被接收但从不使用**；Rust 同样接收后忽略（保持参数形状）。

`commandId` 在写入/更新/删除/启用四个参数里是类型必填，但服务实现**从不读取**。Rust 侧
按可选字段反序列化并忽略，避免为了一个未使用字段拒绝渲染层请求（差异见文末）。

## 目录来源与优先级

用户 home：`HOME`（trim 非空）→ `USERPROFILE`（trim 非空）→ 进程 home。

| agentSource | directorySource | user 目录 | workspace 目录 |
| --- | --- | --- | --- |
| `modeAgent` | `mode` | `{home}/.mode/commands` | `{workspace}/.mode/commands` |
| `modeAgent` | `agents` | `{home}/.agents/commands` | `{workspace}/.agents/commands` |

> TS 的 `SettingsDirectorySource` 还有 `claude`，但 `COMMAND_DIRECTORY_SOURCE_DESCRIPTORS`
> 只有 `mode` 与 `agents` 两项，`claude` 不参与命令发现。Rust 与之一致：**不扫 `~/.claude`**。

`list` 的读取顺序（决定同名去重与展示顺序）：

1. 若给了 `workspacePath`：按 `[mode, agents]` 顺序扫 **project** 目录；
2. 再按 `[mode, agents]` 顺序扫 **global**（user home）目录；
3. 每个 scope 内，`.mode` 是强优先级来源：**该目录读到至少一条有效命令后，同 scope 的
   `.agents` 整段跳过**（`discoveredCount > 0` 即 break，即使 `.mode` 只有一条）；
4. 合并后按 `name`（`/a/b` 形式）**首次出现者胜**去重；
5. 末尾追加插件命令（按名字排序），`commands` 字段是「去重后的用户命令 + 插件命令」。

递归扫描：跳过以 `.` 开头的文件与目录；`lstat` 判定（**不跟随符号链接目录**）；
只收扩展名（大小写不敏感）为 `.md` 的文件；子目录先递归（深度优先、按目录枚举顺序）。

命令 `name`：`/` + 相对命令根的路径（去掉尾部 `.md`），路径分隔符统一为 `/`
（`/a/b`）。`filePath` 是绝对路径原样。命令根是 `scope` 对应的那个根目录（不是 workspace）。

`id`：

- 用户命令：`{agentSource}:{location.source}:{scope}:{name}`
- 插件命令：`plugin:{pluginMarketplace}:{pluginName}:{name}:{filePath}`

`location`：`{source: "mode"|"agents", scope: "user"|"project", directoryPath, projectPath?}`
（`scope` 为 `global` 的命令，location.scope 是 `user`）。

## markdown 解析与生成（逐字对齐）

解析（`parseCommandFile`，任何异常 → `null`）：

1. 按 `\n` 切行（**不处理 `\r`**）。
2. 第一处 `line.trim() === "---"` 的行是起点；起点之后第一处同样的行是终点。两者都找到时：
   终点之前为 frontmatter 行，之后为正文行；**起点之前的行被丢弃**。任一缺失：
   frontmatter 为空、正文为全部行。
3. `prompt` = 正文行 `join("\n")` 后 `trim()`；`content` = 正文行 `join("\n")`（**不 trim**）。
4. `description` / `argument-hint`：遍历 frontmatter 行：
   - 以空格或制表符开头的行不是键；
   - 键 = 对 `line.trim()` 匹配 `^([A-Za-z0-9_-]+)\s*:` 的第一组，**小写化**；
   - 命中目标键后开始收集：该行取 `line.split(":").slice(1).join(":").trim()`，非空则收；
   - 收集期间遇到「非键且以空白开头」的行，收其 `trim()`；遇到**任意**键则停止；
   - 值 = 收集项 `join(" ")` 再 `trim()`，空串 → `undefined`。
5. 解析产物的 `name`（仅内部使用）是文件名去 `.md` 后加 `/`；命令发现时用目录相对路径
   覆盖它。

生成（`generateCommandFileContent`）：

1. 从 `existingContent`（可为空）的 frontmatter 里保留除 `description` / `argument-hint`
   之外的行：命中被替换键后跳过该行及其后续非键行，直到下一个键。
2. 依次追加 `description: {trim}`、`argument-hint: {trim}`（各自 trim 非空才追加）。
3. 保留行数为 0 → 直接返回 `config.prompt`（**无 frontmatter、无换行加工**）。
4. 否则返回 `"---\n" + lines.join("\n") + "\n---\n\n" + prompt`。

文件名（`getCommandFileName`）：`config.name` 去掉**一个**前导 `/`，加 `.md`
（`modeAgent` 的命名分隔符是 `/`，不做 `:` 替换）。

## enable override（`~/.mode/cli/config.json`）

- 路径固定 `{home}/.mode/cli/config.json`。读失败 / 非 JSON / 非对象 → 空对象。
- override 表取 `config.command`：每个「值是对象且 `enable` 是布尔」的条目 →
  `filePath → enable`。键是**命令文件绝对路径字符串**（Windows 反斜杠）。
- `enabled = overrides.get(filePath) ?? true`。
- 写入规则（`setCommandEnabledOverride`）：
  - `enabled = true` → **删除整条** `command[filePath]`（不是写 `{enable:true}`）；
  - `enabled = false` → `command[filePath] = {enable:false}`（覆盖原条目）；
  - `command` 为空对象 → 删除 `config.command`；
  - 其余顶层字段原样保留。
- 写入形状：`JSON.stringify(config, null, 2) + "\n"`，父目录 `mkdir -p`。
- 各方法的 override 副作用：
  - `writeCommandFile`：写完文件后把该路径置为 `true`（删除条目）。
  - `updateCommandFile`：仅当**文件名变了**且旧路径**有** override 时迁移——先把旧路径置
    `true`，再把旧值写到新路径。新文件已存在时按新路径的最新值读取 `enabled`。
  - `deleteCommandFile`：删除文件后把该路径置 `true`（删除条目）。

## 插件命令（只读）

配置（同一份 `~/.mode/cli/config.json`）：

- `plugins.enabled`（布尔，默认 `true`）：`false` → 整个插件命令面为空。
- `plugins.dirs`（字符串数组）：候选根，`marketplace = "inline"`，默认启用。
- `plugins.enabledPlugins`（布尔记录）：键先做 `canonicalPluginId` 归一。
- `plugins.suppressedBuiltins`（字符串数组）：归一后用于跳过官方 cache 里的卸载项。
- `storage.dir`（trim 非空字符串，默认 `~/.mode`）→ 解析为 `resolveConfigPath`；
  basename 是 `cli` 则原样，否则追加 `cli`；再追加 `plugins` 得到插件存储根。

候选根顺序与去重：

1. `plugins.dirs`（marketplace `inline`，默认启用）；
2. 官方 cache 扫描：`{plugins}/cache/mode-plugins-official`，不存在则回退
   `cache/zcode-plugins-official`，再不存在用前者；`<name>/<version>/` 下**每个目录**
   都是候选根（不筛 `.backup` / `.tmp`），marketplace 为
   `mode-plugins-official`，默认关闭；按路径排序；
3. `{plugins}/installed_plugins.json` 的 `plugins[]`：`id`/`marketplace`/`installPath`
   都 trim 非空且 `installPath` 是绝对路径才收；`id` 归一，`marketplace` 取归一后 id 的
   `@` 之后段；默认关闭。

每个候选根读清单（按序取第一个存在的）：
`.mode-plugin/plugin.json` → `.zcode-plugin/plugin.json` → `.claude-plugin/plugin.json` →
`.codex-plugin/plugin.json`。清单必须是对象，`name` trim 后匹配
`^[a-z0-9][a-z0-9._-]{0,127}$`，否则跳过。

- `pluginId = {name}@{marketplace}`；marketplace 是官方 id 且命中
  `suppressedBuiltins` → 跳过；`pluginId` 已出现 → 跳过（首次胜）。
- `defaultEnabled = candidate.defaultEnabled || isDefaultEnabledOfficialPluginId(pluginId)`；
  `enabled = enabledPlugins[pluginId] ?? defaultEnabled`；`false` → 跳过。
- 命令根 = `manifest.commands`（字符串或字符串数组）逐项做**根内包含**校验（绝对路径
  拒绝；解析后必须在根内）；列表为空且该字段**缺失**时，回退到 `{root}/commands`
  （存在才收）。
- 递归扫描：跳过 `.` 开头项，`lstat` 判定目录，`*.md`（大小写不敏感）解析；
  `name` 相对**命令根**；`enabled = pluginEnabled && override(filePath)`；
  同一 `filePath`（反斜杠转 `/` 并小写）只收一次；按 `name` 排序后返回。

## 写路径规则

- `storageLevel` 默认 `user`；`project` 且缺 `workspacePath` → 错误
  `Missing workspace path for project command`。
- 写入根：`storageLevel === "project"` → `{workspace}/{segments}`，否则 user 根。
- `writeCommandFile`：
  1. `mkdir -p` 写入根（**只建这一层**）；
  2. 目标文件已存在 → 错误 `Command file already exists: {fileName}`；
  3. 生成内容 → 原子写（`atomic-file`）；
  4. 清掉该路径的 enable override；
  5. 重新解析写出的内容（解析失败 → `Failed to parse written command file`）；
  6. 返回 `source:"user"`、`enabled:true`、`scope` 与 `projectPath` 齐全的命令。
- `updateCommandFile`：先读旧内容（`oldFilePath` 给定时；读失败当 `undefined`）；
  文件名变化时**先删旧文件**（失败忽略），再做「新文件已存在」检查（仅当新旧路径不同），
  生成时用旧内容保留未知 frontmatter；`mkdir -p` 新文件父目录后原子写；然后迁移 override。
- `deleteCommandFile`：删除文件（`ENOENT` 视为成功），再清 override。
- `getPrimaryUserCommandsDirectory`：`mkdir -p` user 命令根后返回 `{path}`。

**路径逃逸拒绝**：`config.name` 计算出的目标路径必须落在写入根内（规范化后 `..` 不得
越界、绝对路径拒绝），否则报错 `Invalid command file name: {name}`。这是对 TS 的安全加固
（TS 会直接 `join` 并写出根外），只在恶意输入下可观察，见差异表。

## 与 TS 的已知差异

| 项 | TS | Rust | 影响 |
| --- | --- | --- | --- |
| 目标路径逃逸 | `join(root, name)` 可写出根外 | 拒绝（`Invalid command file name`） | 仅恶意 `config.name` |
| 写文件 | `writeFile`（非原子） | `atomic-file`（临时文件 + rename + 锁） | 更强，语义相同 |
| 嵌套命令名首次写 | 父目录不存在时 `ENOENT` | 同样报 `ENOENT: no such file or directory, open '<path>'`（不替用户建目录） | 文案接近，均为失败 |
| 配置键顺序 | 保留插入顺序 | `serde_json` 默认按键排序 | JSON 语义相同 |
| 排序 | `localeCompare` | 确定性比较（小写 → 原文 → 码点） | 仅顺序，见下 |
| 插件 cache 根排序 | `localeCompare` | 码点序 | 仅顺序 |
| 文件读取编码 | UTF-8（非法字节替换 U+FFFD） | `from_utf8_lossy` | 相同 |
| frontmatter `\r` | 不处理（`\r` 留在行内） | 不处理 | 相同 |
| `commandId` 校验 | 类型必填但不用 | 可选、忽略 | 更宽松 |
| 并发 | 无队列 | 无队列（配置写有跨进程锁） | 相同语义 |
| 插件域依赖 | 完整插件域（下载/安装） | 只读已有目录与清单 | 不含安装/市场刷新 |

排序差异只在「名字大小写混排或含标点」时改变展示顺序；命令查找按 `name` 精确匹配，
不受影响。

## 验收清单

- 目录优先级：workspace `.mode` 有命令时跳过 workspace `.agents`；user 侧同理；project
  命令排在 global 之前；同名首次胜。
- enable override：默认启用；`{enable:false}` 关闭；`enable:true` 删除整条；无关条目保留。
- markdown：无 frontmatter、有 frontmatter、起点前有内容、`---` 缩进、多行值、
  大小写键、`argument-hint`、未知键保留、`trim` 与 `content`/`prompt` 的差异。
- 生成：无 frontmatter 时原样返回 prompt；有 frontmatter 时精确字节
  （`---\n…\n---\n\n…`），未知键保留、被替换键删除。
- 写/改/删：首次写、重复写报错、改名（旧文件删除 + override 迁移）、`storageLevel`
  project 缺 `workspacePath` 报错、删除不存在文件不报错。
- 插件命令：`plugins.enabled=false` 空；`dirs`（inline）默认启用；官方 cache 默认关闭
  但 `enabledPlugins[mode-plugins-official]` 可控；`suppressedBuiltins` 跳过；
  `installed_plugins.json` 根；清单四种目录；`commands` 字段为绝对路径/根外时忽略；
  缺省 `commands/` 目录回退；插件文件 override 与 `pluginEnabled` 的与运算。
- 与真实 TS 的黄金对拍：`pnpm commands:fixtures` → `crates/commands-service/fixtures/commands.json`，
  Rust `tests/parity.rs` 覆盖 markdown 解析/生成字节、文件名、list 顺序与字段、
  enable override 序列化结果、插件命令列表。
