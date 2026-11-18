# LT-1 C1：Rust Agent Server 最小闭环 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `subagent-driven-development`（推荐）或
> `executing-plans` 逐任务实现。步骤用 `- [x]` 复选框跟踪。
>
> 本文件是 **C1 检查点**计划，不是 LT-1 全量计划。C1 完成后，C2/C3/C4 各自另出子计划。
> 每个任务开工前先读 `docs/specs/agent-core-parity.md` 的对应章节与
> `docs/specs/tauri-agent-runtime.md` 的 oracle/白名单口径。

**Goal:** 新增 `crates/agent-server`，在 `app-server --stdio` 边界上以 Rust 接管会话：
真实 provider 流、6 个核心工具、权限、会话落库与恢复、显式切换/回滚；通过 oracle 夹具
与真实 provider smoke。

**Architecture:** `agent-process` 继续做进程所有者，命令解析选择 Node `mode.cjs` 或
Rust `agent-server`；`agent-server` 内部用 `agent-core` 的 turn 状态机 + `CommandInbox`，
`provider-http` 做模型流，`session-store` 通过 SQLite WAL + `busy_timeout` 串行化
`db.sqlite` 写（**无**进程独占锁；多 workspace 合法进程可共享同一库）。频道 facade 不变。

**Tech Stack:** Rust 1.99（`rust-toolchain.toml`）、tokio、serde/serde_json、rusqlite
（bundled）、reqwest（rustls + stream）、futures-util、React/TS 仅在测试侧复用。

**Spec:** `docs/specs/agent-core-parity.md`（C1 章节）；行为对拍口径见
`docs/specs/tauri-agent-runtime.md`；协议类型见 `crates/agent-protocol/**`；
数据库 schema 契约见 `scripts/sqlite-schema-baseline.json` 的 `session-store`。

## Global Constraints

- **clean-room**：不读 `apps/mode-cli/**`、`packages/services/**`、`packages/client/**`
  的实现体；只读协议类型声明、测试断言、specs、公开 API 文档。oracle 只黑盒运行。
- **单一所有者**：同一 workspace 一个 Agent；同一会话一个 turn 状态机。`db.sqlite` 不再
  要求进程独占锁：多个合法 workspace 进程可共享同一库，写由 SQLite WAL + `busy_timeout`
  串行化（2026-11-17 控制器裁决 (a)）。C1 不实现任何“同一 workspace 两个运行时同时跑”的兼容模式。
- **显式能力**：C1 不支持的方法返回 `-32601`（facade 映射 `unsupported_method`），
  不返回空结果、不静默降级。
- **不编造观察值**：事件字段/DB 形状只来自协议镜像、schema 基线或 oracle 黑盒夹具；
  未观察到的字段留空并登记。
- **测试先行**：每个任务先写失败用例，再实现；提交前跑本任务的测试 + clippy。
- **不推送**：agent 只做本地提交；push 由人发起。
- **语言政策**：新增代码只用 Rust/TS；夹具用 JSON。

## Review Focus

以下是 C1 最容易被用户碰到的失败模式，每个都要有测试：

1. **双写**：Rust 与 Node 同时为同一 workspace 启动，事件/消息重复或互相覆盖。
2. **半截 turn**：provider 流中断后消息已落一半，`turn.failed` 却缺失或重复。
3. **DB 形状漂移**：`message.data` / `part.data` 与 Node 写入的形状不一致，导致回退读不出。
4. **权限绕过**：`allow_project` 写入的规则行不生效，或 `deny` 仍执行工具。
5. **事件乱序/丢帧**：`session/subscribe` 先发请求后订阅，replay 与 live 之间丢事件。
6. **回滚失败**：移除 Rust 命令后仍残留 Rust 子进程，或 Node 回退读不到 Rust 写的会话。

## File Structure

**新 crate**

- `crates/agent-server/`：`src/main.rs`（入口）、`src/server.rs`（stdio 分帧/分派）、
  `src/runtime.rs`（会话注册表 + CommandInbox + 事件总线）、
  `src/provider_config.rs`（账号 Overlay + Personal 配置解析）、
  `src/methods/*.rs`（session 方法）、`src/reverse.rs`（反向请求）、
  `src/state.rs`（运行时快照/投影）。
- `crates/session-store/`：`src/lib.rs`、`src/error.rs`、`src/schema.rs`（只校验，不建表）、
  `src/session.rs`、`src/message.rs`、`src/part.rs`、`src/usage.rs`。
- `crates/provider-http/`：`src/lib.rs`（trait 复用 `agent-core::ModelProvider`）、
  `src/openai_chat.rs`、`src/sse.rs`、`src/error.rs`。

**修改**

- `crates/agent-core/src/{provider,session,stream}.rs`：provider 异步化 + 流式 sink。
- `crates/agent-core/src/context.rs`（新）：上下文装配 v0。
- `Cargo.toml`：workspace members 增加三个 crate；workspace dependencies 增加
  `futures-util` 已有、`reqwest` 已有；不新增不必要的依赖。
- `crates/agent-process/src/registry.rs`：健康面加 `runtime` 字段（`rust|node`）。
- `docs/specs/tauri-agent-runtime.md`：登记 C1 未产出事件与合法差异。
- `docs/specs/agent-core-parity.md`：C1 完成后勾选并补实际差异。

**测试/夹具**

- `crates/agent-server/tests/{framing,dispatch,lifecycle}.rs`
- `crates/session-store/tests/{schema,crud,transaction}.rs`
- `crates/provider-http/tests/{sse,openai_chat}.rs`
- `crates/agent-core/tests/{turn_stream,provider_script}.rs`（异步化后更新）
- `packages/desktop/tests/agent-runtime-oracle/fixtures/*.json`（新增会话生命周期/落库夹具）
- `docs/plans/2026-11-16-lt1-c1-rust-agent-server.md`（本文件）

---

### Task 0: 黑盒契约捕获（oracle 存储/协议）

**Files:**

- Create: `docs/specs/agent-server-c1-contract.md`
- Create: `packages/desktop/tests/agent-runtime-oracle/fixtures/session-lifecycle/*.json`
- Create: `packages/desktop/tests/agent-runtime-oracle/probe-session-lifecycle.mts`
- Create: `packages/desktop/tests/agent-runtime-oracle/session-lifecycle-fixtures.mts`
- Modify: 无（`record.mts`/`fixtures.mts` 未改）

**Interfaces:**

- Consumes: `driver.mts` 的 `startOracleRuntime`、隔离数据根约定
  （`MODE_DATA_BASE_DIR` + `HOME`/`USERPROFILE`/`MODE_HOME`）。
- Produces: 会话生命周期夹具（create/send/close/resume 的事件序列）+
  `agent-server-c1-contract.md` 里的 DB 行形状/方法顺序表。

- [x] **Step 1: 用隔离数据根录一次「文本 turn + Read 工具 turn + close + resume」**

  命令：`pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-session-lifecycle.mts`
  （`record.mts` 无生命周期 case，已按任务说明落只读探针；夹具放子目录，避免被
  `fixtures.mts` 当成三层行为夹具）。
  结果：夹具 JSON 落盘，`expect.events` 来自 oracle，未手写；探针连跑两次逐字节相同。

- [x] **Step 2: 导出 `db.sqlite` 的 `session/message/part/session_input/session_entry` 行**

  用 Node `node:sqlite` 只读查询，把行形状写进 `agent-server-c1-contract.md`；
  未复制 TS 源码。

- [x] **Step 3: 写失败回放用例**

  `--replay` 在 `agent-server` 不存在时红（exit 1），红因写入 contract doc §5；
  真实三层回放留给 Task 10。

- [x] **Step 4: 提交**

  `5f114327b test(agent): capture C1 oracle lifecycle fixtures`

**Acceptance:** 夹具与形状表可复现；没有手写期望值；隔离根断言通过。

**观察修正（2026-11-16）**：`session_input` 在 create + immediate send 生命周期里 0 行；
spec §5.1/§5.5 已按观察修正。runtime-preferences 请求只在 create/resume 各触发一对。

---

### Task 1: `agent-server` stdio 骨架

**Files:**

- Create: `crates/agent-server/Cargo.toml`
- Create: `crates/agent-server/src/{main,server,dispatch,lifecycle}.rs`
- Modify: `Cargo.toml`（workspace members）
- Test: `crates/agent-server/tests/{framing,dispatch,lifecycle}.rs`

**Interfaces:**

- Produces:
  - `pub async fn run_stdio<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(reader: R, writer: W, server: Arc<AgentServer>) -> Result<(), ServerError>`
  - `pub enum Frame { Request { id: RequestId, method: String, params: Value }, Notification { method: String, params: Value }, Response { id: RequestId, result: Value }, Error { id: RequestId, code: i64, message: String, data: Option<Value> } }`
  - `AgentServer::dispatch(method: &str, params: Value) -> Result<Value, ServerError>`（未知方法 `-32601`）
- Consumes: `serde_json`、tokio。

- [x] **Step 1: 写失败的 framing 测试**

  `crates/agent-server/tests/framing.rs`：LF 分帧、CRLF 容忍、U+2028 不切帧、空行忽略、
  EOF 残留帧处理。先断言未实现。

- [x] **Step 2: 实现最小 `run_stdio`**

  循环读行 → 解析 JSON → 分派 → 写回复；未知方法回 `-32601`；EOF 退出 0。

- [x] **Step 3: 写失败的 dispatch 测试**

  `dispatch("session/unknown", {})` 返回 `-32601`；非法 JSON 关闭连接并失败 pending。

- [x] **Step 4: 实现 `AgentServer` 空壳与 `main.rs`**

  `main` 只接受 `app-server --stdio`（其余参数忽略但记录）；启动 tokio runtime。

- [x] **Step 5: 跑测试并提交**

  `cargo test -p agent-server && cargo clippy -p agent-server --all-targets -- -D warnings`

**Acceptance:** 二进制能被 `tokio::process::Command` 拉起，发一帧收一帧，EOF 干净退出。

---

### Task 2: `session-store`（schema 校验 + 核心 CRUD）

**Files:**

- Create: `crates/session-store/Cargo.toml`
- Create: `crates/session-store/src/{lib,error,schema,session,message,part,usage}.rs`
- Modify: `Cargo.toml`
- Test: `crates/session-store/tests/{schema,crud,transaction}.rs`

**Interfaces:**

- Produces:
  - `SessionStore::open(path: &Path) -> Result<SessionStore, StoreError>`
  - `SessionStore::ensure_supported_schema(&self) -> Result<(), StoreError>`（检查必需表 +
    `schema_migration` 账本；缺表/缺账本 → `StorageSchemaUnsupported`）
  - `create_session(&self, record: SessionRecord) -> Result<()>`
  - `get_session(&self, session_id: &str) -> Result<Option<SessionRecord>>`
  - `list_sessions(&self, workspace_key: &str, include_archived: bool, limit: Option<usize>) -> Result<Vec<SessionRecord>>`
  - `append_message(&self, message: MessageRecord) -> Result<()>`
  - `append_part(&self, part: PartRecord) -> Result<()>`
  - `messages(&self, session_id: &str, after: Option<&str>, limit: Option<usize>) -> Result<Vec<MessageWithParts>>`
  - `append_entry(&self, entry: SessionEntryRecord) -> Result<()>`
  - usages: `record_model_usage/turn_usage/tool_usage`
- Consumes: `rusqlite`、`serde_json`。

- [x] **Step 1: 写失败的 schema 测试**

  用 `scripts/sqlite-schema-baseline.json` 生成测试库 DDL（只读 data 文件）；缺
  `schema_migration` 账本时 `ensure_supported_schema` 必须报 `StorageSchemaUnsupported`。

- [x] **Step 2: 实现 `SessionStore::open` + schema 校验**

  只校验，不建表；打开 `busy_timeout`，不抢写锁。

- [x] **Step 3: 写失败的 CRUD 测试**

  create → get → list；append message/part → messages 顺序/limit；不存在的会话返回 `None`。

- [x] **Step 4: 实现 CRUD 与事务**

  `append_message` + `append_part` 在同一事务；失败回滚；`message.data`/`part.data` 保留
  原始 JSON（serde_json::Value）。

- [x] **Step 5: 写失败的存储并发测试**（多 store 共享同一库；SQLite 串行化写）

  第二个 `SessionStore` 以写模式打开同一文件时 fail-closed（`storage_locked`）。

- [x] **Step 6: 跑测试并提交**

  `cargo test -p session-store && cargo clippy -p session-store --all-targets -- -D warnings`

**Acceptance:** 现有 Node 建的库能读；缺表/缺账本 fail-closed；消息/part 往返逐字节稳定。

---

### Task 3: provider 异步化 + 流式 turn 循环

**Files:**

- Modify: `crates/agent-core/src/provider.rs`
- Modify: `crates/agent-core/src/session.rs`
- Modify: `crates/agent-core/src/stream.rs`
- Test: `crates/agent-core/tests/{provider_script,turn_stream}.rs`

**Interfaces:**

- Produces:
  - `pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;`
  - `pub type DeltaSink = tokio::sync::mpsc::UnboundedSender<ModelDelta>;`
  - `pub trait ModelProvider: Send + Sync { fn stream(&self, request: ModelRequest, deltas: DeltaSink) -> BoxFuture<'static, Result<ModelResponse, ProviderError>>; }`
  - `pub async fn Session::run_turn(&mut self, prompt: &str, provider: &dyn ModelProvider, sink: &mut dyn EventSink) -> Result<(), TurnError>`
  - `pub async fn Session::run_pending_notifications(...)`
- Consumes: tokio sync/select。

- [x] **Step 1: 把 `provider_script` 改成异步测试并让它失败**

  `#[tokio::test]`；断言 delta 顺序与 `ModelResponse` 统计与旧实现一致。

- [x] **Step 2: 改 trait 与 `ScriptedProvider`**

  `stream` 把剧本 delta 逐条 `send` 到 sink，返回 usage/stop；剧本仍由请求内容决定。

- [x] **Step 3: 改 `run_turn` 为流式消费**

  `tokio::select!` 同时消费 delta 与 provider future；delta 到达即发
  `model.streaming`；provider 结束后继续工具批次/下一轮；终态恰好一次。

- [x] **Step 4: 更新既有测试**

  `turn_stream.rs`、`task_notification.rs`、`task_control.rs` 等改为 `#[tokio::test]`；
  事件顺序断言不变。

- [x] **Step 5: 跑测试并提交**

  `cargo test -p agent-core && cargo clippy -p agent-core --all-targets -- -D warnings`

**Acceptance:** 脚本 provider 的 delta 在返回前就能被 sink 观察到（不是收集完再发）；
所有既有 agent-core 测试绿。

---

### Task 4: OpenAI Chat Completions 适配

**Files:**

- Create: `crates/provider-http/Cargo.toml`
- Create: `crates/provider-http/src/{lib,openai_chat,sse,error}.rs`
- Modify: `Cargo.toml`
- Test: `crates/provider-http/tests/{sse,openai_chat}.rs`

**Interfaces:**

- Produces:
  - `pub struct OpenAiChatConfig { pub base_url: String, pub api_key: String, pub model: String, pub max_output_tokens: Option<u64>, pub extra_headers: Vec<(String, String)> }`
  - `pub struct OpenAiChatProvider { /* reqwest::Client + config */ }`
  - `impl ModelProvider for OpenAiChatProvider`
  - `pub fn build_chat_request(request: &ModelRequest, config: &OpenAiChatConfig) -> serde_json::Value`
- Consumes: `reqwest`（stream）、`serde_json`、`futures-util`。

- [x] **Step 1: 写失败的 SSE 解析测试**

  覆盖 `data:` 多行、`[DONE]`、空行、CRLF、非法 JSON、usage 尾帧。

- [x] **Step 2: 实现 SSE 行解析器**

  纯函数：`parse_sse_line` / `SseDecoder::push`；不依赖网络。

- [x] **Step 3: 写失败的 OpenAI 流测试**

  本地 `TcpListener` 返回固定 SSE：文本 delta、工具调用 delta（index/id/arguments 分片）、
  finish_reason、usage。断言发出的 `ModelDelta` 序列与 `ModelResponse`。

- [x] **Step 4: 实现 `OpenAiChatProvider`**

  `POST {base_url}/chat/completions`，`stream:true`、`stream_options.include_usage`；
  401/403/5xx 映射为 `ProviderError { kind, status, message }`；工具入参 JSON 解析失败
  产生显式错误（不静默丢调用）。

- [x] **Step 5: 跑测试并提交**

  `cargo test -p provider-http && cargo clippy -p provider-http --all-targets -- -D warnings`

**Acceptance:** 文本与工具调用都能流式产出；usage 与 finish reason 正确；错误可诊断。

---

### Task 4b: Provider 配置与凭据解析

**Files:**

- Create: `crates/agent-server/src/provider_config.rs`
- Modify: `crates/agent-server/Cargo.toml`
- Test: `crates/agent-server/tests/provider_config.rs`

**Interfaces:**

- Produces:
  - `pub struct ProviderConfigHub { /* account snapshot + personal file */ }`
  - `ProviderConfigHub::handle_account_config(params: Value) -> Result<Value, ProviderConfigError>`
    （应答 `provider/updateAccountConfig` 的 `{receivedRevision, providerCount, status}`）
  - `ProviderConfigHub::resolve(selection: &ModelSelection) -> Result<ResolvedProvider, ProviderConfigError>`
  - `pub struct ResolvedProvider { provider_id, model_id, base_url, api_key, api_format, max_output_tokens, context_window }`
- Consumes: `provider-config`、`provider-config-source`、`provider-config-repository`、
  `provider-resolver`、`account-provider-overlay`、`provider-source-chain` 的公开入口；
  **不复制**这些 crate 的解析逻辑。
- 数据来源：`provider/updateAccountConfig` 下发的账号 Overlay +
  `MODE_PERSONAL_PROVIDER_CONFIG_FILE`；不连接官方平台。

- [x] **Step 1: 写失败的解析测试**

  account Overlay + Personal 文件合并后，`providerId/modelId` 解析出 base URL、API key、
  `apiFormat`；缺 key/缺 URL 返回显式错误；不出现官方端点。

- [x] **Step 2: 实现 `provider/updateAccountConfig` 回执**

  严格解析 `{revision, basedOnModeBuiltinRevision, providers, states}`；revision 去重；
  回执 revision 不一致时不标记已交付。

- [x] **Step 3: 实现 `resolve`**

  复用 `provider-source-chain` / `provider-resolver` 的公开 API；Personal 优先规则与
  Rust 侧既有 `AgentProviderSource` 一致；`apiFormat` 映射到 `provider-http` 适配器。

- [x] **Step 4: 写失败的安全测试**

  `Debug`/日志不打印 API key；错误文案只带 provider/model 与缺失字段。

- [x] **Step 5: 跑测试并提交**

  `cargo test -p agent-server provider_config && git commit -m "feat(agent-server): provider config"`

**Acceptance:** 真实 provider smoke 之前，`session/create` 的模型选择能解析出可用端点；
账号/personal 合并与 Rust 既有事实链一致；无官方平台请求。

---

### Task 5: 上下文装配 v0

**Files:**

- Create: `crates/agent-core/src/context.rs`
- Modify: `crates/agent-core/src/lib.rs`
- Test: `crates/agent-core/tests/context.rs`

**Interfaces:**

- Produces:
  - `pub struct ContextInput<'a> { pub cwd: &'a str, pub mode: &'a str, pub tools: &'a [ToolSpec], pub messages: &'a [ModelMessage], pub model: &'a ModelCapabilities }`
  - `pub fn build_system_prompt(input: &ContextInput<'_>) -> String`
  - `pub fn build_model_request(input: &ContextInput<'_>) -> ModelRequest`
- Consumes: `agent-tools::ToolSpec`、`ModelMessage`。

- [x] **Step 1: 写失败的系统提示快照测试**

  断言包含工作目录、模式、工具名；不包含凭据/内部地址；提示词在本仓库内定义
  （不复制上游）。

- [x] **Step 2: 实现最小系统提示**

  结构固定：身份/工作区/工具使用/安全边界/输出约定；工具描述从 `ToolSpec` 生成。

- [x] **Step 3: 写失败的请求装配测试**

  工具 schema 形状为 OpenAI function tools；消息顺序 = 历史 + 本轮输入；空历史不伪造
  assistant 消息。

- [x] **Step 4: 实现 `build_model_request` 并提交**

  `cargo test -p agent-core context && git commit -m "feat(agent-core): context assembly v0"`

**Acceptance:** 同一输入生成稳定 JSON；无敏感信息；工具 schema 可被 provider 消费。

---

### Task 6: 会话运行时（CommandInbox + 事件总线 + 持久化）

**Files:**

- Create: `crates/agent-server/src/runtime.rs`
- Create: `crates/agent-server/src/state.rs`
- Modify: `crates/agent-server/src/lib.rs`
- Test: `crates/agent-server/tests/runtime.rs`

**Interfaces:**

- Produces:
  - `pub struct SessionRuntime { /* session, inbox, event log, store */ }`
  - `SessionRuntime::new(record, store, provider, config) -> Result<Self>`
  - `SessionRuntime::submit(input: SessionInput) -> Result<Accepted>`
  - `SessionRuntime::subscribe(after_seq: Option<u64>) -> (Vec<SessionEvent>, broadcast::Receiver<SessionEvent>)`
  - `SessionRuntime::snapshot() -> SessionStateSnapshot`
  - `SessionRuntime::cancel_turn() -> Result<()>`
- Consumes: `agent_core::{Session, CommandInbox}`、`session_store`、`provider_http`。

- [x] **Step 1: 写失败的准入测试**

  连续两次 `submit`：第一次 running，第二次 queued；取消后队列按 spec 处理；
  同一 `inputId` 幂等。

- [x] **Step 2: 实现 per-session worker**

  一个 worker task 串行消费 inbox；turn 运行期间 `submit` 只入队；`cancel` 通过
  `CancellationToken` 中断。

- [x] **Step 3: 写失败的事件/持久化测试**

  脚本 provider 跑一个文本 turn：事件顺序 `turn.started → model.streaming → turn.completed`；
  `message`/`part` 行写入；`turn_usage` 行写入；终态只一次。

- [x] **Step 4: 接入 `Session::run_turn` + `session-store`**

  user message 在 turn 开始事务写入；assistant/tool parts 增量写入；终态更新 session/usage；
  失败回滚未提交部分。

- [x] **Step 5: 写失败的订阅测试**

  `subscribe(after_seq)` 先拿到 replay，再拿 live；无重复、无缺口；晚订阅者不丢终态。

- [x] **Step 6: 跑测试并提交**

  `cargo test -p agent-server runtime && cargo clippy -p agent-server --all-targets -- -D warnings`

**Acceptance:** 一个会话的 turn 串行、事件有序、落库可恢复；并发/取消/幂等有测试。

---

### Task 7: session 方法分派与快照

**Files:**

- Create: `crates/agent-server/src/methods/{mod,create,send,subscribe,read,messages,list,close,resume,setters}.rs`
- Modify: `crates/agent-server/src/dispatch.rs`
- Test: `crates/agent-server/tests/methods.rs`

**Interfaces:**

- Consumes: `agent_protocol::{session_create,session_write,session_read,snapshot}` 的严格类型；
  `SessionRuntime`。
- Produces: 方法表 `session/create|send|subscribe|read|messages|list|close|resume|setModel|setThoughtLevel|setMode`。

- [x] **Step 1: 写失败的 `session/create` 测试**

  严格参数（多键拒绝）；返回 `SessionStateSnapshot` 必填字段；落 `session` 行；
  未知可选字段缺省省略。

- [x] **Step 2: 写失败的 `session/send` 测试**

  `accepted:true` 立即返回；错误参数不拉起 turn；`inputId` 幂等；`expectedRevision`
  冲突返回显式错误。

- [x] **Step 3: 写失败的 read/messages/list/close/resume 测试**

  冷读（无活跃 runtime）走 `session_store`；close 后 resume 恢复历史；list 的
  `includeArchived`/`limit` 语义；messages 的 `afterMessageId`/`limit`。

- [x] **Step 4: 实现方法 handler**

  每个 handler 只做参数校验 + 调 runtime/store；不复制 turn 语义；未知方法保持 `-32601`。

- [x] **Step 5: 跑测试并提交**

  `cargo test -p agent-server methods && git commit -m "feat(agent-server): session methods"`

**Acceptance:** 协议方法往返可用；快照形状与 `agent-process-fixture` 的最小快照一致；
未支持方法显式 `-32601`。

---

### Task 8: 权限与反向请求

**Files:**

- Create: `crates/agent-server/src/reverse.rs`
- Modify: `crates/agent-core/src/permission.rs`（如需暴露 handler）
- Test: `crates/agent-server/tests/permissions.rs`

**Interfaces:**

- Produces:
  - `ProtocolPermissionHandler`：把 `PermissionRequest` 作为反向请求发给 Host，等待
    `PermissionResponse`；超时/关闭 → deny。
  - `ProtocolRuntimePreferences`：`session/requestRuntimePreferences` 默认应答。
- Consumes: `agent_core::PermissionHandler`、`PermissionUpdate`。

- [x] **Step 1: 写失败的 allow_once 测试**

  工具触发审批 → 反向请求帧 → 回 `allow_once` → 工具执行 → `permission.resolved` +
  `tool.updated(result)`。

- [x] **Step 2: 写失败的 allow_project/deny 测试**

  `allow_project` 写 `local_setting` 行并在下一次同规则请求直接放行；`deny` 只留
  `scheduled` + `batch(errorCount=1)`。

- [x] **Step 3: 实现 handler 与反向请求 id 管理**

  反向请求 id 单调；关闭时失败所有 pending；未知方法回 `-32601`。

- [x] **Step 4: 写失败的 runtime preferences 测试**

  默认值、非法 scope `-32602`；不读真实账号。

- [x] **Step 5: 跑测试并提交**

  `cargo test -p agent-server permissions && git commit -m "feat(agent-server): permissions"`

**Acceptance:** 三态审批正确；规则持久化可重放；反向请求不泄漏凭据。

---

### Task 9: 切换/回滚与健康面

**Files:**

- Modify: `crates/agent-process/src/registry.rs`（`RuntimeHealth.runtime`）
- Modify: `crates/agent-process/src/resolve.rs`（如需记录选中来源）
- Test: `crates/agent-process/tests/{resolve,health}.rs`
- Test: `crates/agent-server/tests/switch.rs`

**Interfaces:**

- Produces: `RuntimeHealth { runtime: RuntimeKind }`（`rust` / `node`）；
  `AgentCommandSource` 到 `RuntimeKind` 的映射。
- Consumes: 现有 `resolve_agent_command`、`AgentRuntimeRegistry`。

- [x] **Step 1: 写失败的 resolve 测试**

  `MODE_AGENT_SERVER_COMMAND` 指向 Rust 二进制时 `source=Env` 且 `runtime=rust`；
  未设置时 dev dist/源码/打包 Node 为 `runtime=node`；native binary 为 `rust`。

- [x] **Step 2: 实现 `runtime` 字段与日志**

  启动日志 `agent runtime=rust|node`；健康面只读返回；不改变 facade 方法。

- [x] **Step 3: 写失败的切换测试**

  用 `tokio::process::Command` 以 `MODE_AGENT_SERVER_COMMAND` 拉起 `agent-server`，
  发 `session/create`；断言没有 Node 子进程（按命令行过滤）；移除 env 后解析回 Node。

- [x] **Step 4: 写失败的退出/孤儿测试**

  stdin EOF → 进程退出；kill 父进程后子进程被回收（复用 `agent-process` 的进程测试）。

- [x] **Step 5: 跑测试并提交**

  `cargo test -p agent-process -p agent-server && git commit -m "feat(agent): switch/rollback"`

**Acceptance:** 两个开关独立；回滚不迁移数据；无孤儿；健康面可见 owner。

---

### Task 10: oracle 对拍、真实 provider smoke 与文档（已完成 2026-11-17）

**Files:**

- Modify: `packages/desktop/tests/agent-runtime-oracle/*.test.mts`（接入 Rust server）
- Create: `scripts/agent-server-smoke.mts`（opt-in，读取用户提供的 endpoint/model/key）
- Modify: `docs/specs/tauri-agent-runtime.md`（C1 差异表）
- Modify: `docs/specs/agent-core-parity.md`（C1 勾选）

- [x] **Step 1: 接入三层对拍**

  用同一份比较器跑 `read-*` / `write-*` / `edit-*` / `bash-*` 夹具（C1 不产出 `task-*` 夹具）；
  Rust server 用脚本 provider 回放，三层全绿。**实际结果（Task 11）**：38/38 三层全绿；
  `session-stream.json` 是投影快照，显式跳过并留痕。

- [x] **Step 2: 记录合法差异**

  C1 未产出的 `session.updated` / `streamRecovery.updated` / compact/goal 等写入差异表；
  不伪造。

- [x] **Step 3: 真实 provider smoke（opt-in）**

  `scripts/agent-server-smoke.mts` 需要显式 env（endpoint/model/key）；完成文本 + Read
  工具 + 最终回答；断言事件、usage、DB 行；默认 CI 不跑。

- [x] **Step 4: 更新文档与验收记录**

  `agent-core-parity.md` C1 勾选；`tauri-agent-runtime.md` 差异表；本文件状态改为完成。

- [x] **Step 5: 全量门禁**

  `cargo clippy --all-targets -- -D warnings`、`cargo test --workspace`、
  `pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed`；记录真实结果。

**Acceptance:** 三层对拍绿；真实 smoke 有人执行并记录；文档差异完整；门禁真实通过。

---

## 执行顺序与依赖

```text
Task 0（夹具）
   └─> Task 1（stdio 骨架）
         └─> Task 2（session-store）
               └─> Task 6（运行时）
Task 3（异步 provider）──> Task 4（OpenAI 适配）
Task 4b（provider 配置）──> Task 5（上下文）──> Task 7（方法）
Task 1 + 2 + 6 ──> Task 7
Task 7 ──> Task 8（权限）
Task 1 + 9（切换）可与 2/6 并行
Task 10 在 4/4b/5/7/8 后
```

**可并行**：Task 3/4/4b/5（agent-core/provider 线）与 Task 1/2（server/storage 线）互不
改同一文件；Task 9 只改 `agent-process` 与测试。

## 完成定义

- 本文件所有复选框勾选；`docs/specs/agent-core-parity.md` C1 勾选。
- `agent-server` 能被 `agent-process` 以 `MODE_AGENT_SERVER_COMMAND` 拉起，完成
  create/send/subscribe/read/messages/list/close/resume。
- 脚本 provider 三层对拍绿；真实 provider smoke 有人执行。
- 切换/回滚、无孤儿、单写者、终态恰好一次均有测试。
- `cargo clippy --all-targets -- -D warnings`、`pnpm typecheck`、`pnpm lint`、
  `pnpm architecture:check --changed` 真实通过。
- 未支持方法/未产出事件在差异表登记，不静默降级。
