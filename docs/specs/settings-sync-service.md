# Spec：外部 Agent 配置同步（Rust `settings-sync-service`）

状态：2026-10-31 首版。对应 `packages/services/src/settings-sync/settingsSyncService.ts`
的六个方法，频道 `settings-sync` 已在壳内注册（默认不接管）。

## 1. 范围

| Rust                                          | 说明                                                       |
| --------------------------------------------- | ---------------------------------------------------------- |
| `get_claude_agents_file_migration_status`     | `~/.claude/CLAUDE.md` → `~/.mode/AGENTS.md` 的可迁移状态   |
| `copy_claude_agents_file_to_mode_agents_file` | 复制（可选覆盖）                                           |
| `detect`                                      | 扫描外部 agent 的 skills / commands / plugins / mcpServers |
| `import_selected`                             | 按选择复制/链接到 Mode 目录并写配置                        |
| `get_first_run_prompt_state`                  | `setting.json` 的首启提示标记                              |
| `mark_first_run_prompt_handled`               | 写入首启提示已消费                                         |

## 2. 来源与目标

- 外部来源表与 TS 的 `SUPPORTED_*_AGENT_SOURCES` 逐条一致（15 个 agent × skills /
  commands / plugins，12 个 MCP 来源），路径拼接为 `home/global` 与
  `workspace/project` 两种作用域。
- Mode 目标根：
  - skills：`{home}/.mode/skills`、`{workspace}/.mode/skills`
  - commands：`{home}/.mode/commands`、`{workspace}/.mode/commands`
  - plugins：`{home}/.mode/plugins`、`{workspace}/.mode/plugins`，并写
    `{home}/.mode/cli/config.json`（或 `{workspace}/.mode/config.json`）的
    `plugins.dirs`
  - MCP：同一配置文件的 `mcp.servers`
- 作用域：`global` ↔ user 根；`project` ↔ workspace 根（没有 workspace 时不可导入）。

## 3. detect

- **只有 `intent === "manualImport"` 才扫描**；`firstRun` 恒返回空结果。
- 一次调用只扫描**一个**分类，顺序 `skills` → `commands` → `plugins` → `mcpServers`
  （取 `categories` 中第一个命中项），与 TS 一致。
- 结果：`{ agents: [{ agent, discovered, categories: [{ category, discoveredCount,
importableCount, skippedCount, sourcePaths, sourceRoots, selectedByDefault }] }] }`；
  `agents` 只保留 `discovered=true` 的项，顺序同来源表。
- `sourceRoots` 按 `scope`、`path` 升序；组内条目按 `name` 升序。
- 跳过原因：技能/命令/插件为 `targetExists` / `sameNameExists`；MCP 只有
  `sameNameExists`。插件在目标配置缺失时按 `targetExists` 处理。
- MCP 读取格式：
  - `mcpServersJson`：顶层 `mcpServers`；
  - `mcpJson`（OpenCode）：顶层 `mcp`，命令数组折叠为 `command` + `args`，
    `type: local|remote` 映射为 `stdio|http`；
  - `codexToml`：TOML 的 `mcp_servers`（或 `mcpServers`）。
    读取时去掉 `timeout` / `startup_timeout_sec`。

## 4. importSelected

- 每个 selection 处理一个 agent × 分类；未知分类记一条 `skipped` 任务。
- 支持 `sourceScope` / `targetScope` 覆盖与 `skillPaths` / `commandPaths` /
  `pluginPaths` / `mcpServerPaths` 精确筛选（按来源路径字符串）。
- 目标作用域缺省取候选自身作用域；缺失（project 但没有 workspace）→ `skipped`。
- 落地方式：`copy` 递归复制（目标存在即失败）；`symlink` 目录用 junction
  （Windows）/ 软链（Unix），命令文件在 Windows 用硬链接、失败回退复制。
- 插件导入后把目标目录追加到配置 `plugins.dirs`（按 resolve 后路径去重）。
- MCP 导入写入目标配置 `mcp.servers`，保留其余配置内容。
- 任务状态：`failedCount > 0` → `failed`；否则 `importedCount > 0` → `success`；
  否则 `skipped`。汇总为 `successCount/skippedCount/failedCount` 与 `taskResults`。

## 5. Claude AGENTS.md 迁移

- 状态：`supported = sourceExists`；缺失时 `unavailableReason: "missingSource"`。
- 复制：源缺失 → `skipped/missingSource`；目标存在且未要求覆盖 →
  `skipped/targetExists`；否则复制并返回 `overwritten = targetExists`。

## 6. 首启提示

- 读：`setting.json` 的 `settingsSyncFirstRunPromptHandled === true`。
- 写：`update({ settingsSyncFirstRunPromptHandled: true })`。
- 壳内由 `settings_store::SettingsService` 提供读写（与 `setting` 频道同路径、同原子写）。

## 7. 已知差异

1. **YAML**：技能 frontmatter 只解析顶层标量（name/version），嵌套结构忽略；
   非缩进行缺少 `:` 视为严格失败后回退宽松正则——与 npm `yaml` 在罕见输入上可能不同。
2. **JSON 键序**：写回的 `setting.json`/`config.json` 按 serde_json 字母序，
   TS 保留插入序；对拍时 JSON 文件按键排序比较。
3. **TOML 日期时间**：转换为字符串（`chrono` 语义），TS 的 smol-toml 返回 Date 对象；
   配置里出现日期字段属未覆盖边界。
4. **符号链接模式**：Windows 目录用 junction、命令文件用硬链接（失败回退复制），
   与 TS 相同；Unix 用软链。对拍只覆盖 `copy` 模式（避免平台差异）。
5. **错误文本**：复制/文件系统失败在 TS 里抛异常（通道返回 error），Rust 侧
   技能/命令/插件导入记为 `failed` 条目，更接近任务结果语义。

## 8. 验收

- `pnpm settings-sync:fixtures` 生成 `crates/settings-sync-service/fixtures/settings-sync.json`
  （6 个用例：skills/commands/plugins+MCP 探测、copy 导入与二次探测、工作区作用域导入、
  AGENTS.md 迁移与首启状态）。
- `cargo test -p settings-sync-service` 回放同一份文件树与操作序列，逐项深比较结果与结束树。
- `crates/host-channels/tests/settings_sync_channel.rs` 覆盖传输层。
