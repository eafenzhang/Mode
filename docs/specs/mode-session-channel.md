# Spec：`mode-session` 频道（Rust readSession facade）

状态：2026-11-22 新建（收尾 mode-session 迁移）。

## 背景

renderer 对 `mode-session` 只剩 `readSession` 一个调用（其余三个纯转发方法已切到
`mode-agent`，见 LT-16 计划）。Node 的 `mode-session` 服务在 `readSession` 上还包了
两层逻辑，不能直接改指 `mode-agent`：

1. `modeSessionApiRetry`：把上次读到的 `runtime.apiRetry` 暂存在 Host 进程内，
   后续快照缺该字段时补回，切回运行中 task 时重试提示不丢失；
2. `importedClaudeHistoryRepair`：Claude 导入 session 的空历史/反序/旧
   `msg_import_*` 串会话修复（读取 legacy 快照或原 Claude jsonl 后按同 sessionId
   幂等回填 `importedHistory`）。

本频道只实现 `readSession`（其余方法显式 unsupported），供 renderer 走原频道名，
从而可以删除 Node 的 mode-session 服务。

## 1. 方法

`readSession` 入参（`ModeSessionReadParams`）：

```
{workspacePath, workspaceIdentity?, remoteSessionId?, sessionId,
 deliveryKind?, messageLimit?, afterSeq?}
```

处理顺序：

1. `ensure_provider_config(workspace)`（与 `mode-agent.readSession` 同口径）；
2. `read_session_with_policy(StartIfNeeded, deliveryKind, messageLimit, afterSeq)`；
3. apiRetry 暂存/回填（§2）；
4. Claude 导入修复（§3），成功时返回修复后的新快照。

其余命令（createSession/resumeSession/list/…）返回
`mode-session channel: unknown or not-yet-migrated command <name>`。renderer 已不再
调用它们；如出现隐藏调用方，按真实需求再补。

## 2. apiRetry 暂存

按 `workspaceKey\0sessionId` 记录 `Option<ApiRetryStatus>`：

- 快照 `runtime.apiRetry` 为 `Some(v)` → 记录 `v`，快照原样返回；
- 快照状态为 `completed`/`error` → 记录 `None`，并把快照补成 `apiRetry: null`；
- 否则若该 key 有记录 → 把记录补回 `runtime.apiRetry`；无记录则原样返回。
- TS 侧 `trackApiRetryFromSessionEvent` 无调用方，故本实现只做快照驱动的暂存
  （与 TS 当前实际行为一致）。

## 3. Claude 导入修复

触发条件（全部满足才修）：

- 快照状态不是 `running`，且 `runtime.activeTurnId` 为空；
- sessionId 以 `claude-import-` 开头，或消息里存在旧版 `msg_import_<n>` id；
- 无 assistant 消息，或首条消息是 assistant，或存在旧版 `msg_import_<n>`。

历史解析（先 legacy 后原生 jsonl，按 assistant 条数取多者）：

- legacy：`<data_root>/v2/sessions/<sha256(workspaceKey)[:12]>/<taskId>.json` 与
  `<taskId>.deleted.json`；要求 `meta.migrationSource === "claudeCode"`。
- 原生：`claude_import::scan_importable_sessions({workspacePath})` 中
  `build_imported_task_id(workspacePath, sessionId) == taskId` 的候选，再用
  `claude_import_persist::parse_session_file` 完整解析。

修复动作：`createSession` 同 sessionId、`persistence: "immediate"`、
`importedHistory: { source: "claudeCode", title?, createdAt?, updatedAt?, messages }`，
mode/model/thoughtLevel 取原快照。修复失败只记 warn 并返回原快照（fail-soft；
Node 实现会让 readSession 直接 reject）。

## 4. 装配

- `host_channels::mode_session::ModeSessionChannel`：依赖 `AgentControlClient` 与
  `data_root`（legacy 快照路径）；
- 进入 inprocess 默认集与 server-mode 批量注册；未注入 agent control 时不可构造；
- Node 服务保留为回退（`MODE_HOST_INPROCESS_CHANNELS=none`）。

## 5. 已知差异

1. 修复失败 fail-soft（见 §3），Node 为 reject。
2. 历史选择用 Rust 解析器（已与 TS `parseClaudeNativeSessionFile` 对拍），
   legacy 文件只按所需字段宽容解析，不做完整 schema 校验。
3. `mcpServers` 不参与 readSession 修复（TS read 路径同样不传）。

## 6. 验收

- `cargo test -p host-channels --lib mode_session`：shouldRepair 矩阵、
  apiRetry 暂存/清理、legacy 快照解析与来源择优。
- 真机：Claude 导入会话打开、运行中重试提示、普通会话读取。
