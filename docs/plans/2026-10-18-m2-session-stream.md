# M2 Task 5：会话流、投递语义与后台通知 turn Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: `subagent-driven-development`（推荐）或
> `executing-plans` 逐任务实现。步骤用 `- [ ]` 复选框跟踪。
>
> 本文件是 `docs/plans/2026-10-16-tauri-agent-runtime.md` 的 **Task 5 子计划**。
> 阶段权威仍是路线图 M2 节；本文件只补「怎么落地」。
>
> **开工前必读**：阶段 spec `docs/specs/tauri-agent-runtime.md` 的 §1（oracle 契约）、
> §3（合法差异）、§4（mock provider）、§5（M2 边界）、§6（已知误差）；
> Task 4 子计划 `docs/plans/2026-10-17-agent-shell-inbox.md`（后台任务与通知的已观察事实）；
> `AGENTS.md` 的「进程、协议与远程控制」一节（双链路必须区分）。

**Goal:** 把 `agent-core` 从「单 turn 工具批处理」推进成**可流式产出、有终态、能接收后台
任务通知**的会话循环，并把 `desktop-continuous` 与 `web-remote-replayable` 的边界钉清楚。

---

## 现状与口径修正（读代码前先读这里）

| 事实 | 现状 | 影响 |
| --- | --- | --- |
| `agent_core::Session` 只有 `run_tool_batch` | 没有模型层、没有 `turn.completed` / `model.streaming` / 快照 | Task 5 的主体是**新增会话循环**，不是改现有函数 |
| `turn.completed` 需要模型统计 | `tokenCount` / `usage` / `cacheStats` 只有模型层知道 | 必须先有 provider 抽象，否则只能编数字（spec §3 禁止） |
| stdio oracle 只发 `desktop-continuous` | 观察值（spec §1.2） | 不能拿 stdio oracle 直接对拍 `web-remote-replayable` |
| `web-remote-replayable` 的 snapshot/queue 在 Host/relay 层 | 类型在 `packages/shared/src/task-realtime.ts`（`task_snapshot_invalidated` / `task_stream_mirror_batch`）；AGENTS.md 把两条链路的所有权分开写明 | **重放归谁实现是架构分叉**，见 DR-T5-1 |
| 后台任务完成会**自动开新 turn** | Task 4b 已观察（spec §3.4），但通知注入未实现 | Task 5 必须接上 `TaskRegistry` 的完成信号 |

---

## Scope（先说清楚）

**做：**

- 5A：流式 turn 循环——`run(prompt)` 产出 `SessionEvent`，含 `turn.started` /
  `model.streaming` / 工具事件 / `turn.completed|failed` 的**完整事件序列**；
- 5B：投递语义抽象——事件带 `deliveryKind`；`desktop-continuous` 的实时语义先落地；
  `web-remote-replayable` 按 DR-T5-1 的裁决落地或明确移交；
- 5C：后台任务完成 → 注入 `<task-notification>` 的新 turn（Task 4b 的收尾）。

**不做：**

- 真实模型供应商适配的全部细节（DR-T5-2：先 trait + 脚本化 fake，HTTP 适配另立任务）；
- 插件 / MCP / Skills 宿主（M3 sidecar）；
- UI 渲染与手机远控 UI（M4）；
- host 层 `CommandInbox` 的跨层排队（DR-T4-7 仍待人裁决）。

---

## 决策记录（开工前先读）

### DR-T5-1：`web-remote-replayable` 的重放归谁实现（**待裁决**）

| 选项 | 含义 | 代价 |
| --- | --- | --- |
| **(a) 归 Host/relay（建议）** | Rust 运行时只按消费方要求的 `deliveryKind` 发事件；快照 + 队列 + 游标由 M3 的 host/relay 层实现（现有 `task-realtime` 类型就是这一层） | Rust 侧只需一个 sink 接口；重放语义与现有 TS 层不重复 |
| (b) 归 `agent-core` | `agent-core::replay` 维护快照与队列，Host 只转发 | 与 M3 的 task-realtime 重复实现；违反 AGENTS.md「不重复状态」 |

**建议 (a)**：stdio 运行时的观察面从来只有 `desktop-continuous`；把重放做进
`agent-core` 会让同一套快照/队列在 Rust 与 Host 各有一份。Task 5B 先只定义
`EventSink`（是否保留历史由实现决定），重放接线放 M3。

### DR-T5-2：模型 provider 的边界（**建议按此执行，不需另行拍板**）

Task 5 先落 `ModelProvider` trait + **脚本化 fake**（同 oracle 的 mock provider 思路），
它的输出是 `model.streaming` 事件与终态统计。真实 HTTP 供应商适配（Anthropic SSE 解析、
重试、计费头）另立任务——理由：事件契约没定死之前写 HTTP 客户端会返工，而 clean-room
要求我们不照抄旧 provider 实现。

### DR-T5-3：通知 turn 的准入（**已由观察修正**）

**观察（2026-10-18，spec §3.5）**：后台任务完成时若会话**空闲** → 立刻开通知 turn；
若**有 turn 在跑** → **没有通知 turn**（turn 结束后空闲 20s 仍无，未观察到重试）。
即 oracle **不排队、直接丢弃**。

**决定**：M2 的对拍行为按 oracle 实现（不排队）。若要改成「忙时排队」，必须作为**有意差异**
单独登记并由 spec 批准——不能拿产品直觉覆盖对拍基线。host 层 `CommandInbox`（DR-T4-7）
仍是另一个概念，不要混谈。

### DR-T5-4：快照形状不自行发明

若最终要碰 `web-remote-replayable` 的快照，形状只能来自
`packages/shared/src/task-realtime.ts` / `mode-protocol-v4` 的**类型声明**与既有测试；
没有观察依据的字段留空并登记，不编一个「看起来合理」的快照。

---

## Task 0：观察面（先做，阻塞后续）

**Files:**
- Modify: `docs/specs/tauri-agent-runtime.md`（新增 §3.5 会话流观察）
- Create: `packages/desktop/tests/agent-runtime-oracle/probe-session-stream.mts`
- Test: 复用 `pnpm exec tsx --test packages/desktop/tests/agent-runtime-oracle/*.test.mts`

- [x] **Step 0.1：录一次「纯文本 turn」的完整事件序列**（2026-10-18）：
      顺序见 spec §3.5；`turn.started.turnNumber` 首个是 **0**（已修 Rust 侧并加单测）。
- [x] **Step 0.2：录一次「工具 turn」的完整序列**（2026-10-18）：工具事件插在
      `tool_call` 之后、下一次 `text_delta` 之前；另外发现长跑 Bash 的
      `tool.updated kind:"progress"`（约 1s 一条）尚未建模。
- [x] **Step 0.3：观察后台通知 turn 与正在运行的 turn 相遇**（2026-10-18）：
      **没有通知 turn**（20s 内无重试）→ oracle 不排队、直接丢弃；DR-T5-3 已据此修正。
- [x] **Step 0.4：盘点 `web-remote-replayable` 的类型**（2026-10-18）：见 spec §3.5 的表；
      所有者是 Host/relay 层，M2 不复制。
- [x] **Step 0.5：结果写进 spec §3.5**（2026-10-18）。

**Step 0 的产物**：spec §3.5 + 可复现探针；没有观察到的形状不写进实现。

---

## Task 1：`ModelProvider` trait 与脚本化 fake

**Files:**
- Create: `crates/agent-core/src/provider.rs`
- Modify: `crates/agent-protocol/src/session_event.rs`（补 `tool.updated` 的 `progress` kind，
  载荷形状见 spec §3.5）
- Test: `crates/agent-core/tests/provider_script.rs`

**Interfaces:**
- Produces:
  - `trait ModelProvider { fn stream(&self, request: ModelRequest) -> ModelStream; }`
  - `ModelRequest { messages, tools, ... }`（只放事件契约需要的字段）
  - `ModelStream` 产出 `ModelDelta`（文本 / 工具入参）与 `ModelUsage`（token / usage / cache）
- 约束：fake 的剧本来自请求内容（同 TS mock provider 的确定性口径），不读真实文件、不计费。

- [x] **Step 1：先写失败的用例**——同一剧本两次运行产出逐字相同的事件序列。
- [x] **Step 2：实现 trait 与 fake。**
- [x] **Step 3：跑通并提交。**（2026-10-18：`crates/agent-core/src/provider.rs` +
      `tests/provider_script.rs`；`agent-protocol` 的 `progress` kind 也在同批落地。）

---

## Task 2：流式 turn 循环（desktop-continuous）

**Files:**
- Create: `crates/agent-core/src/stream.rs`
- Modify: `crates/agent-core/src/session.rs`（把 `run_tool_batch` 收进循环）
- Test: `crates/agent-core/tests/turn_stream.rs`

**Interfaces:**
- Produces: `Session::run_turn(&mut self, prompt, sink: &mut dyn EventSink) -> TurnOutcome`；
  `EventSink { fn emit(&mut self, event: SessionEvent); }`。
- `SessionEvent.delivery_kind` 由调用方指定；`seq` 单调。

- [x] **Step 1：对拍纯文本 turn**（2026-10-18：`run_turn` + `turn_stream.rs`）。
- [x] **Step 2：接上工具循环**（复用 Task 2–4 的实现；工具批次后自动进入下一轮模型请求）。
- [x] **Step 3：`turn.completed` 的 `resultType` 与统计字段按观察填充**
      （usage 的 10 个键、`tokenCount` = input+output 累计、`historyRoundCount` = 模型请求数）。
- [x] **Step 4：跑通并提交。**
      **登记差异**：oracle 的 `session.updated` / `streamRecovery.updated` 尚未产出
      （需要消息计数与追踪 id）；本步只保证 `turn.started` / `model.streaming` /
      `tool.updated` / `turn.completed` 的顺序与载荷。

---

## Task 3：投递语义边界（按 DR-T5-1）

- [ ] **Step 1：`desktop-continuous` 的负向用例**——晚订阅者拿不到已发事件，不做补发。
- [ ] **Step 2：若裁决 (a)**：只保留 `EventSink` 抽象 + `delivery_kind` 参数，
      在 spec 登记「重放归 Host/relay」；若裁决 (b)：另出 `replay.rs` 子计划，不在本任务里
      静默实现。

---

## Task 4：后台完成通知 turn（5C）

**Files:**
- Modify: `crates/agent-tools/src/task.rs`（完成信号）
- Modify: `crates/agent-core/src/session.rs`（空闲开 turn / 忙时排队）
- Test: `crates/agent-core/tests/task_notification_turn.rs`

- [x] **Step 1：`TaskRegistry` 提供完成订阅**（2026-10-18：mpsc channel，终态迁移时只发一次；
      `take_notifications()` 非阻塞取走）。
- [x] **Step 2：会话循环收到通知 → 组 `<task-notification>` 文本 → 开 turn**
      （`Session::run_pending_notifications`；只在空闲时调用，忙时丢弃由调用方保证——
      与 oracle 一致，见 DR-T5-3）。
- [x] **Step 3：对拍**——`turn.started.input` 与 spec §3.4 的 XML 逐字一致；
      停止任务的通知是 `killed` + `was stopped`；失败是 `failed with exit code 7`。
- [x] **Step 4：提交。**
      **登记差异**：`description` 缺失时的 `<summary>` 文案未观察，当前退回命令原文；
      XML 内容未做转义（观察样本里没有需要转义的字符）。

---

## Task 5：双链路 E2E 与门禁

- [ ] **Step 1：`desktop-continuous` 一套 E2E**（事件顺序 + 断线语义）。
- [ ] **Step 2：按 DR-T5-1 的裁决补 `web-remote-replayable` 的验证面**（若 (a)，验证
      `delivery_kind` 透传与 Host 的既有重放测试；若 (b)，跑 `replay.rs` 的队列用例）。
- [ ] **Step 3：改 stream / snapshot / queue 时必须双跑**（路线图风险对冲）。
- [ ] **Step 4：`cargo clippy -D warnings` + `cargo test` 全绿；提交。**

---

## 验收

1. 纯文本 turn 与工具 turn 的事件序列与 oracle 对拍通过（第三层事件流可以开始比）；
2. `turn.completed` 的统计字段来自 provider，不编造；
3. 后台任务完成通知按观察注入新 turn（含 `killed` / `failed` / `completed` 三种文案）；
4. 双链路语义有明确所有者与各自用例；`deliveryKind` 不会混用；
5. `CommandInbox` 若仍未裁决，必须继续显式标记未实现，不拿会话层排队冒充 host 队列。

## 风险

| 风险 | 对冲 |
| --- | --- |
| 真实 provider 绑定过早，事件契约返工 | DR-T5-2：先 trait + fake，HTTP 另立任务 |
| 把重放做进 agent-core，与 Host 重复 | DR-T5-1 建议 (a)，并由 ledger 裁决 |
| 通知 turn 与用户输入竞争 | DR-T5-3 的排队语义 + 顺序用例；不与 host `CommandInbox` 混谈 |
| 事件流第三层对拍引入时间字段噪声 | 沿用 spec §3 的时间白名单，只比规范化后的形状 |

## 执行记录（2026-10-18 巡检轮）

- **Step 0 全部完成**：新增 `probe-session-stream.mts`（纯文本 turn / 工具 turn /
  通知与运行中 turn 三个用例）；结论写入 spec §3.5。
- 发现并修正：`turn.started.turnNumber` 首个应为 **0**（新增
  `crates/agent-core/tests/session_turn.rs`）。
- 发现但未做：`tool.updated kind:"progress"`（约 1s 一条）尚未在 `agent-protocol` 建模，
  归 Task 1 的前置。
- 关键结论：后台任务在另一个 turn 运行期间完成时，oracle **不排队、直接丢弃**通知
  （spec §3.5）；DR-T5-3 已据此从「排队」改为「按 oracle 丢弃」。

### 2026-10-18 巡检轮（Task 1）

- `agent-protocol` 补 `tool.updated / kind:"progress"` 的载荷
  （`ToolProgressPayload` + `ToolProgressOutputPreview`），往返测试用 spec §3.5 的实测样本。
- `agent-core::provider` 落 `ModelProvider` trait、`ModelRequest` / `ModelResponse` /
  `ModelDelta` / `ModelUsage` 与 `ScriptedProvider`：下标 = `Assistant` 消息条数，
  工具步产出 `tool_input_start → delta → end → tool_call` 的固定顺序。
- 测试：`provider_script.rs` 4 条（选步、delta 顺序、确定性、空剧本报错）；
  `agent-protocol` 的 progress 往返 1 条。

### 2026-10-18 巡检轮（Task 2）

- `crates/agent-core/src/stream.rs`：`EventSink`（`Vec<SessionEvent>` 直接实现）+ `TurnError`。
- `Session::run_turn(prompt, provider, sink)`：`turn.started` → `model.streaming` ×N →
  （工具批次：`tool.updated` scheduled/started/result/batch）→ 下一轮模型流 → `turn.completed`；
  消息历史在 `Session` 内存里维护，工具结果回写给下一轮请求。
- `turn_stream.rs` 2 条：纯文本（3 个事件、tokenCount=22）与工具 turn（11 个事件、
  tokenCount=52、`historyRoundCount=2`）。
- **已登记差异**：未产出 `session.updated` / `streamRecovery.updated`。

### 2026-10-18 巡检轮（Task 4 / 5C）

- `TaskRegistry` 增加完成通知 channel：reaper 在 `completed` / `failed` 迁移时发一条，
  `TaskStop` 在 `killed` 时发一条；`take_notifications()` 非阻塞取走。
- `TaskNotification::to_xml()` 按观察拼 `<task-notification>`（completed / failed / killed 三种文案）。
- `Session::run_pending_notifications` 在空闲时把通知逐条跑成新 turn；忙时丢弃由调用方保证
  （oracle 行为，见 DR-T5-3）。
- `task_notification.rs` 4 条：completed XML、failed summary、killed/stopped、通知 turn 的 input。
