# Spec：Agent 子进程层（`crates/agent-process`）

状态：2026-10-29 实现（W4 最终态第一层：进程所有权迁壳的传输基础）。行为真源：

- `packages/services/src/mode-agent/modeAgentProcessManager.ts`（spawn 参数与生命周期，部分）
- `packages/services/src/mode-agent/modeStdioTransport.ts`（stdio 关闭与进程树清理的参考）
- `crates/sidecar-host/src/process.rs`（无控制台窗口、`taskkill /T /F` 的既有做法）

## 范围

- 按 `AgentProcessSpec`（命令、参数、cwd、env、请求超时）拉起子进程；
- stdin/stdout 接入 `mode-protocol-client`；stderr 逐行广播；
- 优雅关闭：关闭写端（stdin EOF）→ 最多等 `grace` → 超时整树强杀；
- 立即强杀：`kill()`；
- 平台细节：Windows `CREATE_NO_WINDOW`（GUI 壳不弹控制台），Unix `process_group(0)`
  让子进程成为进程组组长，强杀用 `kill(-pgid, SIGKILL)`；
- `kill_on_drop(true)`：调用方忘记 shutdown 时至少回收根进程。

**不在范围**（后续切片）：

- 健康探测、重启监听（注册表已提供显式 `reap_idle`/`shutdown_all` 与周期
  `spawn_idle_reaper`）；
- provider/account 配置同步、`mode-agent` 频道迁移与所有权切换；
- TS 的 stderr drain 窗口、进程树快照（`captureProcessTreeSnapshot`）、stdio tap 开发代理；
- 进程组 id 探测与 Windows `taskkill` 失败后的二次兜底（当前失败只依赖根进程
  `start_kill`）。

## 启动规格解析

`resolve_agent_command` 按 TS `resolveDefaultModeAgentCommand` 的顺序选择命令：

1. `MODE_AGENT_SERVER_COMMAND`（trim 非空）：args 取 `MODE_AGENT_SERVER_ARGS_JSON`
   （必须是字符串数组，非法直接报错；空/缺省回落 `["app-server","--stdio"]`），
   cwd 取 `MODE_AGENT_SERVER_CWD`（trim 非空）否则 workspace；
2. monorepo dev dist：从 `upward_start` 向上找 `apps/mode-cli/packages/cli/dist/mode.cjs`，
   命令为随包 Node（`node_candidates` 第一个存在的文件）否则 `node`；
3. monorepo dev 源码：`apps/mode-cli/packages/cli/src/main.ts` + 向上找
   `node_modules/.bin/tsx`（Windows 优先 `tsx.cmd`）；
4. 打包资源：`<resources>/agent/glm/mode.cjs` + 随包 Node/`node`；
5. 都没有 → `AgentResolveError::NotConfigured`。

cwd 失效时回落到 workspace；再失效则交给 spawn 继承壳进程 cwd。
`presentation_surface == "desktop"` 时移除已有 `--surface`/`--surface=` 并追加
`--surface desktop`（孤立 `--surface` 不吞后续 option，与 TS 修复一致）。

**未移植**：Electron runtime 分支（Tauri 壳没有 Electron）、已部署 native binary 的候选链
（`findModeAgentRuntimeBinary` / `GLM_BINARY_PATH` / `~/.mode/server/agents/glm`）、
`supportsStorageStartup` 标记。native binary 兜底随所有权切换一起补。

## 运行时注册表（单进程不变量）

`AgentRuntimeRegistry` 保证**一个 workspace key 只允许一个 Agent 子进程**：

- key = `workspaceIdentity?.trim() || workspacePath`（与仓库 Workspace Identity 规则一致）；
- `get_or_spawn`：已存在且 transport 未关闭 → 复用；已关闭 → 先回收再重建；
  同一 key 的并发调用只真正 spawn 一次（每 key 一把异步锁），不同 key 互不阻塞；
- `invalidate(key)`：关闭并移除，下次调用重建；
- `reap_idle(older_than, grace)`：只在**没有在飞请求**（`pending_count() == 0`）且
  `idle_duration()` 达到阈值时回收，返回数量；在飞请求结束前绝不回收；
- `spawn_idle_reaper(interval, idle_timeout, grace)`：周期任务，首轮延后一个 `interval`，
  返回 `JoinHandle`，由壳在换代/退出时 abort；
- `shutdown_all(grace)`：壳退出/换代时关闭全部；
- spawn 失败不缓存，下一次调用会重试。

空闲口径来自协议客户端的待处理请求计数：`idle_duration` 在连接建立与每次 pending 归零时
重置（见 `docs/specs/mode-protocol-client.md`）。这与 TS `scheduleIdleReclaim` 的
「在飞请求归零后重新计时」一致。

**当前未做**：健康探测、provider/account 配置同步、进程换代事件。实时切换所有权前必须
先接上这些调用点，并确保 Host 不再拉起同一 workspace 的 Agent。

## 存储启动门

每个 Agent 进程持有自己的 gate，订阅 `startup/storageState` 通知：

- 状态严格必填 `schemaVersion=1`、`attemptId`、`sequence>0`、`databaseId`、
  `databaseKind`（session/tasks-index）、`phase`（checking/waiting_for_lock/migrating/
  committing/ready/failed）、`elapsedMs>=0`；`failed` 必须带 `errorCode`。
- 接受规则：`attemptId`/`databaseId` 不可变、`sequence` 严格递增、`ready`/`failed`
  之后不再接受；非法帧静默忽略（不影响连接）。
- `supports_storage_startup` 只对 env 显式命令为 false；dev dist/源码与打包入口为 true。
  false 时 `prepare_storage` 立即返回（与 TS 的 `requireStorageStartup` 一致）。
- `prepare_storage`：拉起（如需）后等待 `ready`；`failed` →
  `SQLite startup failed: <errorCode>`；首个状态 30s 未到 → `startup_status_timeout`
  并合成一条 failed 快照。
- `getStorageStartupState` 语义：existing-only，返回 `{generation, state?}`；没有活进程
  返回 `None`（不 spawn）。`subscribe_storage_startup` 同样只对活进程可用。
- 与 TS 的差异：状态解析忽略未知字段（不启用 `deny_unknown_fields`）；合成的失败快照
  attemptId/databaseId 用时间戳生成，不追求 UUID 形态。

## Account Provider Config 交付

协议侧对齐 TS `syncAccountProviderConfigToClient`，真实事实源通过 `ProviderSource` 注入：

- 快照：`revision`、`basedOnModeBuiltinRevision`、`providers`（providerId → 已序列化配置）、
  `states`（缺省空表）。
- `sync_account_provider_config(target, source)`：同一 workspace key 串行；已交付 revision
  相同则跳过（返回 `Unchanged`，不发请求）；否则发送 `provider/updateAccountConfig`，
  严格解析 `{receivedRevision, providerCount, status}`。
- 回执 revision 不一致 → `Account Config 接收回执版本与交付版本不一致`，**不**标记已交付，
  下次仍会重试。
- 未注入源 → 直接返回 `None`，与 TS 的 `if (!accountProviderConfigSource) return` 一致，
  也不会拉起 Agent。

**未做**：真实凭据 / OAuth / Family 可用性网络源（`createAccountProviderConfigSource` 的
注入实现）。事实链本身已在 `crates/provider-source-chain` 落地（见
`docs/specs/provider-source-chain.md`）：它组合文件层与账号 Overlay 并直接实现
`ProviderSource`，剩余只是把 `AccountProviderOverlayOptions` 换成生产实现。

## 控制面 facade（第一批：hooks 预信任）

`AgentControlClient` 是 `mode-agent` 频道的第一组方法迁移，先覆盖
`workspace/hooks/trustGrant`：

- `grant_workspace_hook_trust(target, bundleDigest, hookDeclarationDigest)`：
  1. 校验两个摘要均为 64 位小写 hex（与 `modeWorkspaceHookTrustGrantParamsSchema` 一致）；
  2. 通过 `AgentRuntimeRegistry` 取/拉起 Agent（同一 workspace 复用）；
  3. 发送 `workspace/hooks/trustGrant`，参数 `{workspace, bundleDigest, hookDeclarationDigest}`；
  4. 严格解析 `{accepted, reasonCode?}`（`deny_unknown_fields`，未知 reasonCode 报错）。
- `workspace` ref 与 TS `buildWorkspaceRef` 同形：`workspacePath`、可选
  `workspaceIdentity`/`remoteSessionId`、`workspaceKey = identity.trim() || path`；
  `undefined` 字段省略，空字符串保留。
- 错误映射：Agent 返回的 `message` 原样作为可见错误（与 TS 抛 `Error` 一致）；
  非法摘要/非法结果/传输错误分别区分。

## 运行时生命周期 facade（第二组）

与 `IModeAgentService` 的 workspace 生命周期方法一一对应，全部 existing-only 或纯生命周期，
不会为了查询而拉起 Agent：

- `runtime_identity(target)`：返回 `RuntimeIdentity { generation, identity, processId,
  workspaceKey }`；`identity = ${workspaceKey}:${generation}:${pid}`（TS 还有 lane 维度，Rust
  当前没有 lane）。没有活进程返回 `None`，频道层负责映射
  `Mode agent runtime identity is unavailable.`。
- `dispose_workspace(target, grace)`：关闭并移除该 workspace 的 Agent（`disposeWorkspace`）。
- `dispose_all(grace)`：关闭全部（`disposeAll`）。
- `storage_startup_state` / `subscribe_storage_startup`：existing-only 读取/订阅存储门。

## 会话只读组（第三组，进行中）

第一片是 `list_sessions(target, options)`（协议方法 `session/list`）：

- `ListSessionsOptions { include_archived=false, limit?, session_ids?, runtime_policy }`；
  缺省的可选键不会序列化。
- 结果按 `agent_protocol::session_read::SessionListResult`（`deny_unknown_fields`）严格解析，
  失败消息前缀 `invalid session/list result`。
- `RuntimePolicy::StartIfNeeded`（默认）复用或拉起；`ExistingOnly` 只复用活进程，
  取不到时返回 `ControlError::RuntimeUnavailable`（消息与 code 与 TS
  `MODE_AGENT_RUNTIME_UNAVAILABLE` 一致），绝不 spawn。

第二片是 `read_session_events(target, sessionId, afterSeq?, limit?)`（`session/events`）：
事件信封严格解析，载荷保持原始 JSON；失败消息前缀 `invalid session/events result`。

第三片是 `list_session_subagents(target, sessionId, endedCursor?, endedLimit?)`
（`session/subagents`）：`endedLimit` 缺省 20 且恒发；running/ended 严格解析。

第四片是 `read_session_messages(target, sessionId, afterMessageId?, limit?)`
（`session/messages`）：消息/部件按 `role` 与 `type` 判别联合严格解析，`cost` 保留 JSON
数字原样。

第五片是 `read_session(target, sessionId, deliveryKind?, messageLimit?, afterSeq?)`
（`session/read`）：完整状态快照严格解析（settings/projection/runtime/messages + 可选
goalStats/todos/slashCommands）。

第六片是 `read_session_debug(target, sessionId)`（`session/debug`）：existing-only，
只在已有进程上读取诊断快照（rounds/networkEntries/cache）。

## 会话写路径（第一批）

- `create_session(target, options)`：完整 `session/create` 参数（含 MCP server 与导入历史），
  返回新会话快照；灰度布尔只在 true 时下发。
- `send_prompt(target, params)`：`session/send` 提交输入；发出前校验协议的三条组合约束，
  `accepted` 非 true 视为非法结果。
- `close_session(target, sessionId, expectedPersistence?)`：`closed` 缺省按成功处理为 `true`。
- `set_session_model` / `set_session_thought_level` / `set_session_mode`：返回新的完整
  状态快照；缺省的可选参数不序列化。
- 规范与验收见 `docs/specs/session-write-group.md`。

反向请求：`runtime_preferences_handler` / `default_runtime_preferences_handler` 生成
`session/requestRuntimePreferences` 的宿主侧应答；`AgentRuntimeRegistry::set_request_handler`
对现有与后续进程统一生效。

事件订阅：`subscribe_session(target, sessionId, deliveryKind, afterSeq?, includeSnapshot?)`
返回 `SessionSubscription`（replay + 快照 + 实时 `recv()`）；`seq == 0` 补单调序号、
`eventId` 去重、按 session 过滤；`Drop` 中止转发任务。

workspace 只读面：`read_workspace_presentation`（closed-transport 重试一次）与
`list_child_processes`（无 params）。

`mcp/list`：`list_mcp_server_statuses(target, {mcpServers?, mode?})`，结果按严格快照解析
（status/failureKind/authorization 等）。

工作流只读：`list_saved_workflows` / `get_saved_workflow`（Ok/Failure 联合）/
`list_saved_workflow_runs`（limit 1..=50 前置校验）；写侧：
`update_saved_workflow_meta` / `delete_saved_workflow` / `move_saved_workflow`（同样的
Ok/Failure 联合）。

v4 命令最小闭环：`resolve_interaction_v4`（`v4/command` 的 `resolveInteraction`）；
ACK 的 accepted/duplicate/noop 视为成功，其余映射为 `ControlError::CommandRejected`。

插件只读：`list_plugins`（组件分组/hook 详情/诊断）与 `describe_plugin`（组件清单）。

线格式、三态 nullable 字段与验收见 `docs/specs/session-read-group.md`。

**未做**：频道注册与所有权切换（Host 仍是 Agent 所有者）；会话其余读/写方法与插件/
automation/workflow 等按方法组迁移。

## 与 TS 的差异

| 差异 | 说明 |
| --- | --- |
| stderr 排空窗口 | TS 在关闭时等待 stderr 排空（1.8–3.25s）；这里关闭后直接 abort stderr 任务 |
| 进程树快照 | TS 在发 EOF 前保存树成员以便找回 detached 后代；这里依赖 `taskkill /T` 与进程组 |
| 强制终止顺序 | TS 先 `SIGTERM`/关闭再 `taskkill`；这里优雅等待后直接整树强杀 |
| 进程组探测 | 未实现 `ownedProcessGroupId` 的存活校验 |
| 日志 | 本 crate 不落日志；stderr 由调用方消费 |

## 关闭语义

1. `shutdown(grace)`：`ProtocolClient::close_write()` 排队帧 flush 后 drop writer →
   子进程收到 stdin EOF。
2. 在 `grace` 内退出：正常返回；客户端随后因 stdout EOF 关闭。
3. 超时：Windows `taskkill /PID <pid> /T /F`；Unix 对进程组发 `SIGKILL`；再等
   `2s` 回收根进程。
4. `kill()`：跳过优雅等待，直接执行第 3 步。

## 验收

`crates/agent-process/tests/process.rs`（5 条，真实子进程）：

- stdio 往返 + stderr `handled:<method>`；
- 优雅关闭：stdin EOF 后子进程退出，客户端关闭，后续请求 `Closed`；
- `stubborn` 子进程：grace 超时后被强杀，`pid_is_alive` 最终为 false；
- 启动即退出：请求以 `Closed`/`Transport` 失败；
- 不存在的命令：`AgentProcessError::Spawn`。

`crates/agent-process/tests/registry.rs`（9 条）：同 key 复用同一进程、identity 优先于路径、
并发 `get_or_spawn` 只 spawn 一次、transport 关闭后重建、`invalidate`/`reap_idle` 回收、
在飞请求期间不回收、`spawn_idle_reaper` 周期回收、spawn 失败不缓存、`shutdown_all` 关闭全部。

`crates/agent-process` 启动规格单元测试（6 条）：env 覆盖（args/cwd）、非法 args JSON、
dev dist 向上查找与 Node 选择、打包资源分支、全部缺失时 `NotConfigured`、
`--surface desktop` 改写（含移除已有值、非 desktop 不修改）。

`crates/agent-process/tests/control.rs`（9 条）：grant 请求的 workspace ref/摘要逐字段断言、
非法摘要不 spawn、Agent 错误 message 原样上抛、非法结果拒绝、workspace ref 省略可选字段且
workspaceKey trim、同一 workspace 两次 grant 复用同一 Agent、facade 的 prepareStorage
等待 ready 并暴露快照、existing-only runtime identity 与换代 generation、dispose_all 清空。

`crates/agent-process` 存储门单元测试（5 条）：单调状态与身份不变、failed 必须带 errorCode、
非法 schemaVersion/sequence 拒绝、ready 放行且非 required 立即返回、首帧超时。

`crates/agent-process/tests/storage.rs`（4 条）：prepare 等待 ready 与快照/订阅、failed 传播
TS 文案、非 storage 入口不等待、失效重建后 generation 递增。

`crates/agent-process/tests/provider_sync.rs`（3 条）：payload 逐字段与 revision 去重、
无源不拉起、回执版本不一致报错且不标记已交付。

`crates/agent-process/tests/session_list.rs`（4 条）：workspace ref 与可选键形状、缺省不序列化、
existing-only 不 spawn 且报 runtime-unavailable、未知结果字段被拒绝。

`crates/agent-process/tests/session_events.rs`（3 条）：游标参数与信封解析、缺省游标不序列化、
事件信封多键被拒绝。

`crates/agent-process/tests/session_subagents.rs`（3 条）：缺省 endedLimit=20、显式游标/分页、
未知结果字段被拒绝。

`crates/agent-process/tests/session_messages.rs`（3 条）：游标与部件解析、缺省不序列化、
未知消息字段被拒绝。

`crates/agent-process/tests/session_read.rs`（3 条）：快照解析与可选参数、缺省不序列化、
未知快照字段被拒绝。

`crates/agent-process/tests/session_debug.rs`（2 条）：existing-only 不 spawn 且已有进程可读、
未知诊断字段被拒绝。

`crates/agent-process/tests/session_write.rs`（10 条）：create 的完整参数与灰度省略、
create 全缺省不出键、create 非法结果、send 完整参数与 ACK、send 组合约束不发请求、
send 的 accepted=false 被拒绝、setModel 参数与快照、thoughtLevel/mode 缺省省略、
close 的 persistence 与默认 true、非法写结果被拒绝。

`crates/agent-process/tests/runtime_preferences.rs`（4 条）：默认应答与 `-32602`/NotHandled、
启动即发的反向请求被应答、非法 scope 回 -32602、已有进程挂新 handler。

`crates/agent-process/tests/session_events_stream.rs`（1 条）：replay 补号/排序、live 去重与
跨 session 过滤、subscribe 参数形状。

`crates/agent-process/tests/workspace.rs`（3 条）：展示结果的 workspace ref 与 slashCommands、
子进程无 params 契约、两类非法结果。

`crates/agent-process/tests/mcp.rs`（3 条）：指定 mode 的状态解析、mode 缺省省略、非法结果。

`crates/agent-process/tests/workflows.rs`（5 条）：list 的 scope 与清单、get 的成功/失败联合、
runs 的 artifacts 与 limit 前置校验、updateMeta/delete/move 的参数与结果、六类非法结果。

`crates/agent-process/tests/v4.rs`（2 条）：resolveInteraction 信封与 ACK、rejected 映射。

`crates/agent-process/tests/plugins.rs`（3 条）：list 的 scope 与组件、describe 的组件分组、
两类非法结果。

`crates/mode-protocol-client` 另有一条 `close_write` 测试：写端关闭后新请求失败、
读端仍能收到通知。
