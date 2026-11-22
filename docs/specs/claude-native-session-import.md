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

## 导入（Stage 2，2026-11-22 已落）

`importClaudeSessions({workspacePath?, workspaceIdentity?, sessionIds})` 流程（真源
`claudeNativeSessionImportService.ts` + adapter 的 `createImportedSession` 路径）：

1. 去重/trim sessionIds；逐个 `findImportableSession`（扫描边界同 Stage 1）。
   未命中 → skipped `session_not_found_or_workspace_mismatch`；workspace 不存在 →
   skipped `workspace_path_missing`；workspace 过滤不一致 → skipped（同上）。
2. 复制源 JSONL → `{dataRoot}/v2/agent-config/claude/{workspaceHash}/projects/…`
   （复制前记录是否已存在；失败清理新建副本）。
3. 完整解析（`claude_import_persist.rs`）：用户轮次/assistant 多事件聚合、synthetic
   占位（`<synthetic>` 且非 API error）过滤并剥离尾部 "No response requested."、
   API error 气泡保留、模型只取首个可见回复、timestamp 毫秒/秒/数字串/ISO。
4. `createSession`：sessionId = `claude-import-<sha256("claude:path:sid")[..24]>`，
   `persistence:"immediate"`，`importedHistory{source:"claudeCode", title,
   createdAt/updatedAt, messages[{role,content,timestamp}]}`；快照经
   `snapshot_to_meta` 校验。
5. legacy 快照 → `{dataRoot}/v2/sessions/{hash}/{taskId}.json`（原子写，2 空格 +
   换行），按 filter 删除 `meta.mode` / `meta.model` / `meta.provider` /
   `messages[].model`；显式 workspacePath 过滤时写入 workspaceIdentity。
6. 索引回写：`migrationSource:"claudeCode"`、`searchableText`（UTF-16 20 万上限）、
   `archived:false`、`deleted:false`（重导入恢复列表可见）；
   发 `workspace_task_list_changed`/`task_meta_changed` 事件。

**失败语义**：解析/复制/创建/落盘/索引任一步失败 → `failed{reason, workspacePath?}`，
并清理本次新建的副本（已创建的 session 不回滚，TS 同）。

**已知差距**：legacy 快照写入前未复用 `parseLegacyTaskSessionFile` 的 zod 校验
（Rust 侧构造形状受控）；`repairImportedClaudeSnapshot`（空历史修复）在
createTask draftSessionId 路径，导入不涉及；需真机验证 `createSession(importedHistory)`
对旧 CLI 的 compat 重试路径。

## 验收

`cargo test -p host-channels --lib claude_import`（5 条：可见候选/预览标题、sidechain/
worktree/workspace 过滤、modifiedSince+limit、head 解析 string/array、标题截断）+
`claude_import_persist`（4 条：轮次聚合/synthetic/API error、任务文件过滤、searchable
文本、复制与 legacy 快照）+ mode-task 频道分发形状/requires-control 用例。
