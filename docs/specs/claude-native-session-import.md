# Spec：Claude Code 原生历史导入（mode-task scan/import）

真源：`packages/services/src/session/claude-native/`（repo / head parser / import parser /
task file / persist）。Rust 实现分两阶段，当前为 **Stage 1（扫描）**。

## 产品规则

- 数据源是 Claude Code 原生目录，不是 Mode 自己的数据根：
  `{HOME|USERPROFILE|MODE_DATA_BASE_DIR}/.claude/projects/**/*.jsonl`（多根去重）。
- 递归收集 `.jsonl`，跳过 `subagents` / `worktree` / `worktrees` 目录（大小写不敏感）。
- 每个文件只读**头部 16 条非空记录**：
  - 任一记录 `isSidechain`（entry/message/request 任一位置）→ 不可导入；
  - cwd 缺失 → 不可导入；
  - cwd 属于 `.claude/worktree(s)` 临时工作区 → 不可导入；
  - 可选 `workspacePath` 过滤：按绝对路径归一化（去尾分隔符、Windows 小写）比较。
- 候选：`{provider:"claude", sessionId=文件名, workspacePath=cwd, sourcePath,
  updatedAt=文件 mtime, createdAt?, previewTitle?}`；`previewTitle` 取首个可见用户
  文本（`deriveSessionTitle`：UTF-16 取前 50 + "..."）；`createdAt` 取该记录时间戳。
- 排序：`updatedAt` 降序；`limit` 为有限正数时截断。
- 可见文本清洗：删除 `<ide_opened_file>…`、`<local-command-…>…`、`<command-…>…`
  标签块（大小写不敏感），`\r\n`→`\n`，trim；tool_result 与 assistant 非 text part
  不进入预览。

## 已知差距（Stage 2）

`importClaudeSessions` 尚未迁移（当前显式 unknown command）：完整解析会话、复制
Claude JSONL 到 `agent-config/claude/{workspaceHash}/projects`、`createSession`
携带 `importedHistory`、写 `.mode-session` legacy 快照（过滤 meta.mode/model/
provider 与 messages[].model）、索引回写（migrationSource=claudeCode、
searchableText、archived:false/deleted:false）与 workspace 生命周期事件。

## 验收

`cargo test -p host-channels --lib claude_import`（5 条：可见候选/预览标题、sidechain/
worktree/workspace 过滤、modifiedSince+limit、head 解析 string/array、标题截断）+
mode-task 频道分发形状用例。
