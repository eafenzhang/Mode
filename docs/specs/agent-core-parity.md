# Spec：Agent Core 行为对齐与 Rust 收口（LT-1）

状态：**已批准方向（2026-11-16，用户选择「LT-1 全量」）**；C1 待实现。
关联：`docs/plans/2026-11-16-upstream-capability-long-tasks.md`（LT-1）、
`docs/plans/2026-11-16-lt1-c1-rust-agent-server.md`（C1 计划）、
`docs/specs/clean-room-policy.md`、`docs/specs/tauri-agent-runtime.md`、
`docs/specs/host-ownership-switch.md`、`docs/specs/agent-process.md`、
`docs/specs/session-read-group.md`、`docs/specs/session-write-group.md`、
`docs/specs/sqlite-schema-gate.md`、`docs/specs/mode-protocol-client.md`。

## 1. 目的

把 Agent 会话运行时从 Node `mode.cjs` 迁到 Rust，并保证：

1. **协议 drop-in**：Rust 进程在同一个 `app-server --stdio` 协议边界上工作，
   `crates/agent-process` 与 `host-channels/mode_agent` 不需要改协议；
2. **行为等价**：按 `tauri-agent-runtime.md` 的三层口径（工具调用序列 / 文件快照 /
   事件流）与 TS oracle 对拍；不完全等价的字段必须显式登记；
3. **单一所有者**：同一 workspace 同一时刻只有一个 Agent 进程；同一会话只有一个
   turn 状态机；同一 `db.sqlite` 只有一个写者；
4. **可回滚**：显式开关切到 Rust；去掉开关回到 Node；切换期间不双写、不丢事件；
5. **可验收**：C1 先交付最小闭环，C4 通过 20 个真实模型任务后才把默认运行时翻到 Rust。

**不是**在本 spec 里新增 Agent 产品功能。LT-1 只改变「谁执行」，不改变「执行什么」。

## 2. 目标架构

### 2.1 进程边界

新增 `crates/agent-server`，产出 `mode-agent` 二进制（名称由打包决定），实现
`app-server --stdio`：

```text
Renderer（草稿 / pending overlay）
   │ session/send（intent）
   ▼
Host rpc-host（路由/租约；不持有 turn 状态）
   │ stdio / RPC 帧
   ▼
agent-process（spawn / 进程树 / stderr / 空闲回收；协议客户端不变）
   ├─ Node `mode.cjs`（oracle / 显式回退）
   └─ Rust `agent-server`（目标默认）
        ├─ agent-core（turn / session / CommandInbox 唯一所有者）
        ├─ agent-tools（工具）
        ├─ provider-http（模型流）
        ├─ session-store（`db.sqlite` 单写者）
        └─ agent-protocol（帧/事件）
   ▼
ProductProjection / SessionEvent（单一序列）
```

选择 drop-in 而不是「壳内直调 `agent-core`」的理由：

- 既有的 102/115 方法、事件桥、健康面、进程回收都能复用，切换成本最低；
- oracle 与 Rust 在同一 stdio 边界上，黑盒对拍不需要第二套驱动；
- 回滚只是命令解析变化，不需要回滚壳内路由；
- 远程/SSH/WSL/Docker 主机同样受益（它们也需要一个可执行 Agent）。

**不做**：把 `agent-core` 直接嵌进 Tauri 壳、另建一套壳内 RPC 面；这会产生第二条
协议路径与第二份事件投影。

### 2.2 关键组件

| 组件                       | 职责                                                          | 不负责                           |
| -------------------------- | ------------------------------------------------------------- | -------------------------------- |
| `crates/agent-server`      | stdio 分帧、方法分派、反向请求、启动/存储门、进程生命周期     | turn 语义、工具语义、存储 schema |
| `crates/agent-core`        | turn 状态机、`CommandInbox`、上下文装配、压缩/记忆、恢复/目标 | 网络/进程/sqlite 细节            |
| `crates/agent-tools`       | 工具纯函数与产物                                              | 会话状态、权限持久化             |
| `crates/provider-http`     | OpenAI/Anthropic 流式适配、工具调用累积、usage 映射           | provider 配置解析、账号/余额     |
| `crates/session-store`     | `db.sqlite` schema 兼容、session/message/part/usage 读写      | turn 状态、事件序列              |
| `crates/agent-process`     | 拉起/持有/回收进程、stdio 客户端、provider 配置交付           | 运行时语义                       |
| `host-channels/mode_agent` | 频道 facade → 控制面方法                                      | 运行时内部状态                   |

## 3. 所有权与事件顺序

### 3.1 会话所有权

```mermaid
sequenceDiagram
  participant R as Renderer
  participant H as Host rpc-host
  participant P as agent-process
  participant A as agent-server (Rust)
  participant S as session-store
  R->>H: session/send
  H->>P: 取 client（workspace 唯一）
  P->>A: ensure spawned / reuse
  A->>A: CommandInbox 串行准入
  A->>S: 追加 user message / session_input
  A->>A: turn 状态机（模型 → 工具 → 模型）
  A-->>P: session/event 通知（有序）
  A->>S: 追加 assistant/tool parts、usage
  P-->>H: 事件帧
  H-->>R: scoped 回帧
```

不变量：

1. **一个 workspace 一个 Agent**：key = `workspaceIdentity?.trim() || workspacePath`
   （沿用 `AgentRuntimeRegistry`）。
2. **一个会话一个 turn 状态机**：`CommandInbox` 是唯一准入队列；Renderer 只保留未提交
   草稿与 pending optimistic overlay。
3. **请求与事件走同一下行路径**：`agent-server` 不直连 Renderer。
4. **终态恰好一次**：`turn.completed` / `turn.failed` 每个 turn 只发一次。
5. **单写者**：同一 `db.sqlite` 只允许一个 Agent 进程写；oracle 对拍使用隔离数据根，
   不与 Rust 运行时共享库。

### 3.2 双链路

- `desktop-continuous`：实时 push；Rust `agent-server` 只负责产生有序事件，
  **不做快照/队列**。
- `web-remote-replayable`：snapshot + gap 恢复归 Host/relay；Rust 只透传
  `deliveryKind` 与 `seq`。
- 两条链路消费同一个 `ProductProjection`，但恢复语义不互相扩散。

## 4. 检查点与范围

| 检查点                  | 交付                                                                                                                                                                   | 明确不做                                                                                                             |
| ----------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------- |
| **C1 最小闭环**         | `agent-server` + OpenAI Chat Completions + `session-store`（已有数据根）+ 6 个工具 + 权限 + create/send/subscribe/read/messages/list/close/resume + setter + 切换/回滚 | MCP/Skills/Plugins/Subagents/Workflows/附件/v4 帧；compact/memory/goal；Anthropic/Responses；fresh-install migration |
| **C2 上下文/记忆/压缩** | 上下文装配、compact、memory、token 预算、会话快照齐全；Anthropic Messages / OpenAI Responses                                                                           | 生态面                                                                                                               |
| **C3 恢复/目标/故障**   | stream recovery、retry、goal/target、后台通知、子代理基础面、终态收口                                                                                                  | 生态面                                                                                                               |
| **C4 生态与默认翻转**   | MCP/Skills/Plugins/Subagents/Workflows/附件/v4；20 任务；默认 Rust；Node 仅显式回退                                                                                    | 新功能                                                                                                               |

C1 的详细任务见 `docs/plans/2026-11-16-lt1-c1-rust-agent-server.md`。

## 5. 行为契约

### 5.1 会话生命周期

C1 必须实现：

| 方法                                               | 语义                                                                                                                 |
| -------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------- |
| `session/create`                                   | 严格解析 `SessionCreateParams`；返回完整 `SessionStateSnapshot`；落 `session` + 初始 `session_entry`。`session_input` 只在 deferred/排队输入时写；create + immediate send 观察到 **0 行**（2026-11-16 oracle 夹具 `session-lifecycle`） |
| `session/send`                                     | 严格解析；`CommandInbox` 准入；返回 `{sessionId, accepted:true, stateRevision}`；turn 异步执行                       |
| `session/subscribe`                                | 先订阅通知再发请求；返回 replay + 可选 snapshot；live 只投递本会话                                                   |
| `session/read`                                     | 从活跃 runtime 或 `session-store` 冷读；返回快照                                                                     |
| `session/messages`                                 | 按 `afterMessageId`/`limit` 返回 `{info, parts}`；part 13 种按协议镜像。无 `after` 且给 `limit` 时取**最新** N 条（与 Node store 的 `limit` 语义一致），结果仍按升序返回 |
| `session/list`                                     | `includeArchived` 恒发；`limit`/`sessionIds` 缺省省略。按 workspace 的 `directory` 精确匹配；`project_id`（`proj_` + 归一化 cwd）不作为 workspace key，避免跨工作区串列表 |
| `session/close`                                    | 关闭 runtime；`persistence` 语义按参数；不删除事实                                                                   |
| `session/resume`                                   | 冷会话恢复为活跃 runtime；返回快照                                                                                   |
| `session/setModel` / `setThoughtLevel` / `setMode` | 更新 runtime 设置并返回快照；`persistAsWorkspaceLastUsed` 按协议省略/下发                                            |
| `session/compact`                                  | C2；C1 返回 `unsupported_method`（显式）                                                                             |
| `session/goal`                                     | C3；C1 返回 `unsupported_method`（显式）                                                                             |

未知方法统一 `-32601 Method not found`，频道 facade 映射为 `unsupported_method`；
**不返回空结果**。

### 5.2 turn 状态机

一次 turn 的顺序（C1 子集）：

```text
accepted → turn.started → model.streaming×N
  →（可选）tool.updated(scheduled/started/result|error/batch)
  →（可选）下一轮 model.streaming
  → turn.completed | turn.failed
```

- `turnNumber` 首个为 0，之后 +1（沿用已观察值）。
- `model.streaming` 的 delta 顺序即 provider 输出顺序；工具入参只出现在
  `kind:"tool_call"`（`scheduled` 带 `inputOmitted:true`）。
- 工具批次内并行执行；完成事件顺序不作为契约（沿用 spec §3 无序组②）。
- `session.updated` / `streamRecovery.updated` 在 C1 可以缺省，但必须在 C1 验收记录里
  登记为「未产出」，不能伪造。
- C1 不产出 `compact` / `goal` / `subagent` / `workflow` 相关事件。

### 5.3 工具与权限

- C1 工具：`Read`、`Write`、`Edit`、`Bash`、`TaskOutput`、`TaskStop`
  （`crates/agent-tools` 已有；新增工具必须先有 oracle 观察）。
- 权限：沿用 `agent-core` 的 `evaluate` + 三态响应（`allow_once` / `allow_project` /
  `deny`）；`allow_project` 写 `local_setting`（与既有 Rust `SqliteRuleStore` 同表）。
- 工具错误：`tool.updated(kind:"error")`，`error.type/message/code` 按协议；
  未注册工具在 `scheduled` 后直接 `error`，没有 `started`。
- 工具产物：`<dataRoot>/.mode/cli/exec/<sessionId>/<toolCallId>-stdout.log`（已有语义）。

### 5.4 事件与协议

- 事件信封复用 `agent_protocol::session_event::SessionEvent`；`eventId`/`sessionId`/
  `turnId`/`seq`/`timestamp`/`deliveryKind`/`type`/`payload` 严格。
- C1 只产出已建模类型；其余类型不编造。
- 反向请求：`session/requestRuntimePreferences`（默认值）、
  `interaction/requestPermission`（三态）；未注册反向请求回 `-32601`。
- `startup/storageState`：启动时按 `agent-process` 的存储门语义发 checking/ready/failed；
  `supportsStorageStartup` 与现有命令解析一致。

### 5.5 持久化与 schema

- 数据库：`{dataRoot}/.mode/cli/db/db.sqlite`（CLI 布局）。
- C1 只接**已由 Node 迁移完成**的数据根；若迁移账本缺失/不匹配，fail-closed 报错，
  不静默建表。C2 再实现 fresh-install migration。
- C1 读写表：`session`、`message`、`part`、`session_entry`、`session_input`、
  `session_target`、`todo`、`local_setting`、`permission`、`model_usage`、
  `turn_usage`、`tool_usage`。`session_input` 只在输入被排队/deferred 时出现；
  immediate send 不写行（oracle 观察，见 `agent-server-c1-contract.md` §3.5）。
- `message.data` / `part.data` 的 JSON 形状以 `agent_protocol::message` 镜像与
  `session-store` 基线为准；C1 先录 oracle 的写入夹具，再按夹具断言。
- 事务：一个 turn 的 user message + parts 在同一事务；assistant/tool parts 按事件
  增量写入；终态更新 session/usage 在同一事务。失败回滚到 turn 前，不留下半截消息。
- 单写者：`agent-server` 打开 DB 时使用独占/进程锁（与 Node 的 WAL 语义一致），
  检测到另一个写者时 fail-closed；oracle 对拍使用隔离根。

### 5.6 provider 适配

- C1：OpenAI Chat Completions 流式（`stream:true`）；解析 SSE `data:` 行、`[DONE]`、
  工具调用 delta（按 index/id 累积）、usage；错误映射为 `turn.failed`。
- C2：Anthropic Messages、OpenAI Responses。
- provider 配置来自 `provider-config` / `provider-resolver`；凭据来自
  `credential-store` 或请求级 `requestAuth`；不连接官方平台。
- `maxOutputTokens` / `contextWindow` 来自模型目录能力；缺失时使用保守默认并记录。
- C1 不做自动重试；C3 再补 retry/backoff（错误必须可诊断，不用超时掩盖）。

### 5.7 上下文、记忆、压缩（C2）

- 系统提示与模式指令由本仓库 spec 定义，**不复制上游提示词**；C1 只落最小系统提示。
- 上下文装配顺序：系统提示 → 模式/工作区事实 → 工具 schema → 技能/记忆（C2）→
  历史消息 → 本轮输入/附件（C4）。
- compact：阈值 = `contextWindow - maxOutputTokens` 的可配置比例；摘要落
  `session_entry`/`message` 投影；compact 失败不阻断下一轮，但必须可见。
- memory recall/write：只在 `memoryEnabled` 时启用；写入失败 fail-soft，不污染 turn。

### 5.8 流恢复、目标、故障（C3）

- stream recovery：断线/重试时从最后一个已提交 part 续传；不重复 delta、不丢工具结果；
  `streamRecovery.updated` 作为锚点。
- goal/target：`set/replace/pause/resume/clear`；停止条件、最大轮次、用户中断；
  `session_target` 是唯一事实源。
- 故障：provider 错误、工具错误、超时、取消各自映射；终态恰好一次；不可恢复错误
  进入 `turn.failed` 并保留已提交 part。
- 后台任务通知：空闲时开新 turn；忙时按 oracle 观察（C3 重新对拍，不凭产品直觉）。

### 5.9 生态面（C4）

- MCP / Skills / Plugins / Subagents / Workflows / 附件 / v4 会话帧：
  复用 LT-2、LT-6 与既有 `crates/mcp-sync-service`、`skills-service`、`plugin-host`、
  `subagents-service`，不在本 LT 重复实现协议。
- 20 任务验收以 `host-ownership-switch.md §7` 为准；每项记录运行日志、事件关键帧、
  失败根因。

## 6. 失败语义

| 场景                                      | 行为                                                                |
| ----------------------------------------- | ------------------------------------------------------------------- |
| provider 401/403                          | `turn.failed`，错误码可诊断；不重试（C3 再按错误分类重试）          |
| provider 流中断                           | 保留已提交 part；C3 从锚点恢复；C1 fail-closed 并标记 turn 失败     |
| 工具异常                                  | `tool.updated(error)`；turn 继续；错误进入下一轮上下文              |
| 权限拒绝                                  | 只留 `scheduled` + `batch(errorCount=1)`；无 `started/result/error` |
| `db.sqlite` 被另一写者持有                | 启动 fail-closed，报 `storage_locked`；不降级为内存                 |
| 迁移账本缺失/不匹配                       | 启动 fail-closed，报 `storage_schema_unsupported`；C2 才支持创建    |
| stdio 关闭                                | 当前 turn 失败、pending 请求统一失败、进程优雅退出；无孤儿          |
| 重复 `session/send`（同 inputId/queryId） | 幂等：不重复开 turn；返回已接受状态                                 |
| stale run                                 | 旧 generation 的事件/结果拒绝写入；按 owner/lease 判定              |

## 7. 切换与回滚

- **命令解析**：`MODE_AGENT_SERVER_COMMAND` / `MODE_AGENT_SERVER_ARGS_JSON` /
  `MODE_AGENT_SERVER_CWD` 优先；未设置时走 dev dist/源码/打包 Node；native binary
  作为打包兜底（`agent-process::resolve` 已有）。
- **频道所有权**：`MODE_HOST_INPROCESS_CHANNELS` 包含 `mode-agent` 时壳内频道生效；
  与命令解析独立，便于「Rust 进程 + Node 频道」和「Rust 进程 + Rust 频道」分开验收。
- **回滚**：移除 Rust 命令 env（回 Node）或移除 `mode-agent` 频道（回 Host）；
  两个开关都必须有测试；回滚不迁移数据（同一 `db.sqlite`）。
- **观测**：启动日志记录 `agent runtime=rust|node`、binary path、generation、
  workspaceKey；健康面 `agent-health` 增加 `runtime` 字段（只读，不改变 facade）。
- **禁止**：Rust 与 Node 同时为同一 workspace 启动；切换期间不共享 DB 写。

## 8. 数据兼容与迁移边界

- 数据根与 CLI 布局沿用 `mode-data-root.md`；`db.sqlite` 的 schema 以
  `scripts/sqlite-schema-baseline.json` 的 `session-store` 为契约，只增不改。
- C1：读已迁移库；缺表/缺账本 → fail-closed。这样 C1 不会在用户真实数据根上
  “顺手建表”。
- C2：实现与 TS 等价的 migration runner（账本、checksum、只增）；fresh install
  由 Rust 创建 schema；用 `pnpm check:sqlite-schema` 与 TS 生产者对拍。
- 消息/part 的 JSON：先用 oracle 写入夹具锁定形状；Rust 写入后由 Node 读取验证
  （双向兼容），再反过来。任何形状差异必须进 spec 的合法差异表。
- 迁移是**单向**的：一旦 C4 默认 Rust，Node 回退只读旧数据；不允许两个运行时同时
  升级 schema。

## 9. 对拍与验收

### 9.1 三层对拍

沿用 `tauri-agent-runtime.md`：

1. 工具调用序列：`name` / 规范化入参 / `ok` / 规范化输出；
2. 文件快照：相对路径 → sha256；
3. 事件流：`{type,payload}` 序列，按白名单归一化；无序组只限状态推送与并行完成事件。

Rust `agent-server` 必须用**同一份**比较器（TS `compare.mts`）或与其逐字节一致的
投影；不实现第二套规则表。

### 9.2 C1 门禁

- 脚本 provider 回放 `read-single` / `write-new` / `edit-*` / `bash-*` / `task-*` 夹具，
  三层全绿。
- 真实 provider smoke：OpenAI 兼容端点，文本 + 一次 `Read` 工具调用；事件、usage、
  落库可复现。
- 会话生命周期往返：create → send → close → resume → list/read/messages；
  `session/message/part` 行与 schema 基线一致；重启后不丢。
- 切换/回滚：Rust 命令下无 Node 子进程；移除命令后 Node 正常；无孤儿。
- `cargo clippy --all-targets -- -D warnings`、`pnpm typecheck`、`pnpm lint`、
  `pnpm architecture:check --changed`。

### 9.3 C4 门禁

- `host-ownership-switch.md §7` 的 20 个真实模型任务全部通过。
- 默认运行时翻到 Rust；Node 只在显式回退时启动；`db.sqlite` 单写者有测试。
- 性能基线：turn 首事件延迟、工具往返、内存占用与 Node 基线对比；不达标项登记。

## 10. 风险与开放决策

| 风险/决策                 | 说明                                                              | 处理                                                            |
| ------------------------- | ----------------------------------------------------------------- | --------------------------------------------------------------- |
| 提示词/上下文无法逐字对拍 | 模型输出非确定；上游提示词受版权约束                              | 只对拍事件/工具/文件；提示词由本仓库 spec 自写；20 任务验功能   |
| DB 消息 JSON 形状         | `message.data` / `part.data` 是行为契约，但未在 spec 里逐字段展开 | C1 第一项任务录 oracle 写入夹具；差异登记                       |
| fresh-install migration   | C1 只读已迁移库；C2 才建库                                        | C2 以 schema 基线 + TS 生产者对拍                               |
| provider API 漂移         | OpenAI/Anthropic 流式细节变化                                     | 适配层用公开 API 文档 + 本地 mock server 测试；不做“猜字段”兜底 |
| 双写/双 owner             | 切换期最大风险                                                    | 进程级单 owner + 单 DB 写者 + fail-closed                       |
| 范围蔓延                  | C1 很容易被 MCP/Skills 拉大                                       | C1 清单写死“不做”；C4 消费 LT-2/LT-6                            |
| 工具面不足                | 20 任务需要 MCP/技能/子代理                                       | 明确属 C4；C1 只验 6 工具                                       |

## 11. Clean-room 声明

- 不读取、不打开、不搜索 `zai-org/ZCode` 与 `apps/mode-cli` / `packages/services` 中
  上游衍生的实现体；只读协议类型声明、测试断言、specs 与公开 API 文档。
- oracle 只作为**黑盒**运行：喂输入、收事件、看文件快照与数据库输出；
  不把旧实现代码粘进提示词或新代码。
- 提示词、上下文模板、工具描述由本仓库自行编写；若出现非接口约定所致的逐字雷同，
  丢弃重写。
- 本 spec 与 C1 计划中的字段/形状均来自协议镜像、schema 基线或黑盒观察；
  未观察到的字段留空或标 `unsupported`，不编造。

## 12. 验收清单

1. C1 计划的任务全部勾选，门禁通过。
2. C2–C4 各自 spec 更新后，检查点验收记录入库。
3. 20 任务通过；Node 回退可用；`db.sqlite` 单写者测试通过。
4. 所有“未产出/未支持”的差异在 `tauri-agent-runtime.md` 的合法差异表或本 spec
   的风险表登记。
5. `pnpm verify:pre-push`、`cargo clippy --all-targets -- -D warnings` 通过。
