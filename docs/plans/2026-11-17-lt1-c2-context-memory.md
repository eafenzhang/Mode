# LT-1 C2：上下文、记忆、压缩与 provider 扩展 Implementation Plan

> **For agentic workers:** 本文件是 C2 检查点的长任务计划。每个 Task 开工前按
> `writing-plans` 补齐该 Task 的逐步骤细节；步骤用 `- [ ]` 跟踪。
>
> 前置：C1 已完成（`feat/lt1-c1-rust-agent-server` @ `c49bd97b6`）。C2 在
> `feat/lt1-c2-context-memory` 上叠加，不重写 C1 已验收的行为。

**Goal:** 在 Rust `agent-server` 上补齐上下文预算、compact、memory recall、Anthropic/Responses
适配与空数据根迁移，使 C2 的会话行为在事件/DB/快照三层与 oracle 一致。

**Architecture:** `agent-core` 负责预算/裁剪/摘要语义；`agent-server` 负责方法、记忆注入、
快照与持久化；`provider-http` 负责两种新 wire；`session-store` 负责空库创建与忠实账本。

**Tech Stack:** Rust 1.99、tokio、serde/serde_json、rusqlite、reqwest（rustls+stream）、
`crates/project-memory`、`agent-protocol`。

**Spec:** `docs/specs/agent-c2-context-memory.md`；总 spec `docs/specs/agent-core-parity.md`；
对拍口径 `docs/specs/tauri-agent-runtime.md`。

## Global Constraints

- **clean-room**：不读 `apps/mode-cli/**`、`packages/services/**`、`packages/client/**`
  实现体；只读协议类型、测试断言、specs、公开 API 文档与黑盒观察。
- **观察优先**：compact/memory/contextUsage/迁移账本的形状只来自 T0 录制；未观察到的
  字段不伪造，登记为差异。
- **单一所有者**：compact 由 per-session worker 串行执行；记忆只读；不新增第二份摘要或
  第二条写入路径。
- **预算不编数**：capabilities 缺失时不做自动 compact，不伪造百分比。
- **回放不回退**：C1 的 38/38 三层回放必须保持；C2 新增夹具另行计数。
- **门禁**：`cargo test`（touch crates）、`cargo clippy --all-targets -- -D warnings`、
  `pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed`；真实结果上报。
- **不推送**：agent 只做本地提交。

## Task 总览

| ID  | 任务                    | 规模 | 依赖                          | 主要落点                                                           | 状态 |
| --- | ----------------------- | ---- | ----------------------------- | ------------------------------------------------------------------ | ---- |
| T0  | C2 黑盒契约捕获         | M    | 无                            | `docs/specs/agent-c2-contract.md`、probes/fixtures                 | 完成（2026-11-17） |
| T1  | 上下文预算 v1           | M    | T0（capabilities/usage 观察） | `agent-core/src/context.rs`、runtime/state                         | 完成（2026-11-17） |
| T2  | Compact                 | L    | T0、T1                        | `agent-core/src/compact.rs`、`agent-server/src/methods/compact.rs` | 完成（2026-11-17，T2a/T2b） |
| T3  | Memory recall           | M    | T0                            | `agent-server/src/memory.rs`、context 注入                         | 完成（2026-11-17，复核 11-18） |
| T4  | Anthropic Messages 适配 | L    | 无                            | `provider-http/src/anthropic_messages.rs`                          | 完成（2026-11-17） |
| T5  | OpenAI Responses 适配   | L    | 无                            | `provider-http/src/openai_responses.rs`                            | 完成（2026-11-17） |
| T6  | 快照/读面对齐           | M    | T1/T2/T3                      | `agent-server/src/{state,methods/projection}.rs`                   | 完成（2026-11-18） |
| T7  | 空数据根迁移            | L    | T0（ledger 观察）             | `session-store/src/{migrations,schema}.rs`                         | 完成（2026-11-18） |
| T8  | 对拍、smoke 与文档      | M    | 全部                          | replay harness、`tauri-agent-runtime.md` §C2                       | 完成（2026-11-18） |

**波次**：Wave 0 = T0；Wave 1 = T1 + T4 + T5；Wave 2 = T2 + T3 + T7；Wave 3 = T6；
Wave 4 = T8。T7 可在 T0 账本录制后与 T2/T3 并行（不同 crate）。

---

### T0: C2 黑盒契约捕获

**Files:**

- Create: `docs/specs/agent-c2-contract.md`
- Create: `packages/desktop/tests/agent-runtime-oracle/fixtures/c2/*.json`
- Create: `packages/desktop/tests/agent-runtime-oracle/probe-c2-*.mts`

**观察项（全部从隔离数据根的真实 oracle 录制，不手写）**：

- `session/compact`：参数、结果、事件序列、`message`/`part`/`session_entry` 行、`time_compacting`；
  手动 compact 与并发第二次的 `already_running`；失败（provider 错误）形态。
- 自动 compact：长历史触发时的阈值/事件/DB；无法触发则登记「未观察」。
- memory：`memoryEnabled` 为 true/false 时模型请求是否包含项目记忆；记忆目录读取；
  是否存在 agent 写记忆路径（工具/方法/模型提取）。
- `session/read` 的 `contextUsage` 字段与数值来源；`settings.model.available` 形状。
- 空数据根：oracle 创建的 `db.sqlite` 的 `sqlite_master`（session-store 部分）与
  `schema_migration` 行（id/checksum/time）。
- Anthropic/Responses 不需要 oracle：以公开 API 文档为契约，T4/T5 用本地 SSE 夹具。

**验收**：contract 文档列「观察值 / 出处 / 未观察」；fixtures 的 `expect` 全部机器录制；
探针可复现；隔离根断言通过。

---

### T1: 上下文预算 v1

**Files:**

- Modify: `crates/agent-core/src/context.rs`
- Modify: `crates/agent-core/src/provider.rs`（如需 usage/预算类型）
- Modify: `crates/agent-server/src/runtime.rs`、`state.rs`
- Test: `crates/agent-core/tests/context.rs`、`crates/agent-server/tests/runtime.rs`

**接口**：

- `ContextBudget { context_window: Option<u64>, max_output_tokens: Option<u64> }`
- `trim_messages(messages, budget) -> TrimmedHistory { messages, dropped, compact_needed }`
  — 保持 tool_call/tool_result 成对；系统提示不参与裁剪。
- `ContextUsage { used, window, percent? }` 投影进快照。

**验收**：预算数学、成对裁剪、未知窗口不自动 compact、`contextUsage` 与 T0 观察一致；
重复装配稳定；测试覆盖边界（0/缺失/超预算）。

---

### T2: Compact

**Files:**

- Create: `crates/agent-core/src/compact.rs`（摘要提示/解析/摘要消息形状）
- Create: `crates/agent-server/src/methods/compact.rs`
- Modify: `crates/agent-server/src/{dispatch,runtime}.rs`、`session-store`（如需摘要行）
- Test: `crates/agent-server/tests/compact.rs`、fixture replay

**接口**：

- `session/compact` 严格参数/结果（`agent_protocol::session_write`），返回
  `{response, snapshot, compact?}`。
- `CompactCoordinator::request(...)`：per-session 单飞；`already_running`；`expectedRevision`
  冲突显式错误。
- 摘要请求复用当前 provider；`instructions` 进入请求；失败保留原历史。

**验收**：T0 夹具回放（事件/DB/快照）；并发/冲突/失败；compact 后下一 turn 历史含摘要；
自动 compact 仅在观察支持时启用；C1 回放保持绿。

---

### T3: Memory recall

**Files:**

- Create: `crates/agent-server/src/memory.rs`
- Modify: `crates/agent-server/src/{runtime,methods/create}.rs`、`agent-core/src/context.rs`
- Test: `crates/agent-server/tests/memory.rs`

**接口**：

- `MemoryContext::load(data_root, workspace_id) -> Result<Option<MemorySection>, MemoryError>`
  使用 `project_memory::ProjectMemoryService` 的公开 API；索引优先、5 MiB、无 symlink。
- `memoryEnabled` 来自 `session/requestRuntimePreferences` 的宿主应答；关闭时零读取。
- 注入为独立「项目记忆（只读）」段；失败 fail-soft。

**验收**：开关、注入内容、缺失目录/超限/损坏 fail-soft、无 symlink；写记忆路径按 T0
观察决定（无观察则登记缺口，不新增工具）。

---

### T4: Anthropic Messages 适配

**Files:**

- Create: `crates/provider-http/src/anthropic_messages.rs`
- Modify: `crates/provider-http/src/lib.rs`、`crates/agent-server/src/provider_config.rs`
- Test: `crates/provider-http/tests/anthropic_messages.rs`

**契约**：见 C2 spec §4.1；公开 API 文档为唯一 wire 依据。本地 `TcpListener` 回放 SSE；
文本/thinking/tool_use/usage/错误；`stop_reason=tool_use` → `"tool-calls"`；无重试。

**验收**：请求映射、流事件顺序、工具入参分片、usage、错误显式；`api_format` 选择适配器。

---

### T5: OpenAI Responses 适配

**Files:**

- Create: `crates/provider-http/src/openai_responses.rs`
- Modify: `crates/provider-http/src/lib.rs`、`crates/agent-server/src/provider_config.rs`
- Test: `crates/provider-http/tests/openai_responses.rs`

**契约**：见 C2 spec §4.2；本地 SSE 回放文本/function_call/usage/完成状态/错误；参数解析
失败显式报错；无重试。

**验收**：请求映射、事件顺序、工具调用、usage、错误；与 chat-completions 共用
`ModelProvider` 接口。

---

### T6: 快照/读面对齐

**Files:**

- Modify: `crates/agent-server/src/state.rs`、`methods/projection.rs`、`methods/mod.rs`
- Test: `crates/agent-server/tests/methods.rs`

**内容**：`settings`/`projection`/`runtime`/`contextUsage` 按 T0 观察补齐；
`settings.model.available` 由 provider 目录投影；未知字段不伪造；差异登记到
`tauri-agent-runtime.md` §C2。

**验收**：schema 解析 + 字段断言；hot/cold 一致；C1 快照测试不回归。

---

### T7: 空数据根迁移

**Files:**

- Create: `crates/session-store/src/migrations.rs`
- Modify: `crates/session-store/src/{lib,schema}.rs`
- Test: `crates/session-store/tests/fresh_install.rs`

**内容**：空根创建 session-store schema + 迁移账本，id/checksum 与 T0 观察逐字节一致；
已安装旧库只读兼容；并发首次启动只创建一个；`pnpm check:sqlite-schema` 通过。
**不读 TS 迁移实现**；账本行来自真实库观察。

**验收**：空根 → 表/索引/账本一致；第二次启动 no-op；并发测试；基线/门禁通过。

---

### T8: 对拍、smoke 与文档

**Files:**

- Modify: `packages/desktop/tests/agent-runtime-oracle/{replay-agent-server,agent-server-replay.test}.mts`
- Create: `scripts/agent-c2-smoke.mts`（opt-in：Anthropic/Responses 真实端点）
- Modify: `docs/specs/tauri-agent-runtime.md`（§C2 差异）
- Modify: `docs/specs/agent-core-parity.md`（C2 勾选）

**内容**：回放 C2 新夹具（compact/memory/contextUsage）；C1 38/38 保持；Anthropic/Responses
本地 SSE + 真实端点 smoke（人工凭据）；差异表收缩；门禁全跑。

**验收**：C2 夹具全绿 + C1 38/38；smoke 有人执行并记录；差异表与代码一致；门禁真实通过。

**完成记录（2026-11-18）**：C2 回放 harness（`replay-c2.mts` + 9 条用例）全绿；
C1 38/38 保持（套件仅余 3 条已登记 session-stream 基线红）；真实端点 smoke 脚本
`scripts/agent-c2-smoke.mts` 就绪（opt-in，env 凭据），**未执行**——需要人工提供
真实端点凭据（§9.8 如实登记）；差异表 `tauri-agent-runtime.md` §C2.8 与代码一致，
并收录 T8 新发现的 C1 时代 DB 层未对齐面。

---

## 执行顺序

```text
T0 ──> T1 ──> T2 ──> T6
  ├──> T3 ────────┘
  └──> T7
T4, T5（独立，可并行）
T8 最后
```

**可并行**：T1/T4/T5 改不同 crate；T2/T3/T7 在 T0 后可并行；T6 依赖 T1/T2/T3。

## 完成定义

- 所有 Task 勾选；C2 spec §9 验收全部记录。
- C1 回放 38/38 保持；C2 新增夹具全绿；compact/memory/provider/migration 测试通过。
- `cargo clippy --all-targets -- -D warnings`、`pnpm typecheck`、`pnpm lint`、
  `pnpm architecture:check --changed` 真实通过。
- 未观察/未支持项在 `tauri-agent-runtime.md` §C2 与 contract 文档登记。
