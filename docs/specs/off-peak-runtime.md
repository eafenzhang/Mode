# Spec：闲时任务调度运行时（Rust `off-peak-runtime`）

状态：2026-11-22 新建（恢复随 Electron 线删除、未重建的派发链）。

> **产品边界（2026-11-22 复核）**：off-peak 已在去智谱化中整体下线——
> `assertOfficialServiceRemoved("offPeak")` 在 `resolveOffPeakCredentials` 与
> `offPeakServerClient.request` 的凭证/网络动作前短路，灰度配置 fail-closed，
> UI 入口隐藏。本 crate 因此没有生产输入（不会有新任务翻到 `schedulable=1`），
> 默认关闭且只由 `cargo test` 驱动；HTTP/凭据/管理频道迁移已取消。是否保留
> 该 dormant 实现待产品裁决。

## 背景

闲时任务的取号/轮询/管理面已在 `crates/off-peak-service` 与 Node Host 提供，但把
`schedulable=1` 的 queued 任务认领并转成会话 prompt 的 scheduler 原在 Electron main
（`packages/desktop/src/scheduler/`、`src/host/offPeakDispatchPlan.ts`、
`src/main/desktopCronScheduler.ts`），随 `d71ed992b`（移除旧 Electron 线）删除且未在
Tauri 重建。实测 `OffPeakTaskRepo.claimDue`/`markRunning` 在 TS/Rust 均无生产调用方，
`off-peak-scheduler-wake-request` 频道无发送者——**off-peak 任务当前不会被执行**。

本 spec 恢复执行侧（不是迁移旧实现）：行为依据是本仓库已冻结的契约
`docs/specs/off-peak-repo.md`、`docs/specs/off-peak-service.md`、
`docs/specs/session-write-group.md`（`offPeakTaskId`/`offPeakRunType`）与
`packages/shared/src/off-peak-types.ts` 的稳定标记。

## 1. 所有权

| 面 | 所有者 |
| --- | --- |
| 取号 / 批量状态轮询 / settle outbox | Node Host `OffPeakTaskService.startSync()`（本次不迁） |
| claim / running / 终态落库 | Rust `OffPeakRuntime`（单实例，Tauri 进程内） |
| 会话创建/恢复与 prompt | 经 mode-task 频道公开方法（`createTask`/`resumeTask`/`sendPrompt`） |
| 终态事件 | 同进程 `TaskEventBus`（G2.1：sessions-index phase 迁移） |

- Rust 只消费 `schedulable=1` 的任务，**不写 ticket 状态**；Node 轮询把票置 ready 后
  写 `schedulable=1`，Rust 下一 tick 认领。
- Node 侧没有 scheduler，因此不需要「停 Node 调度器」；同一时刻只有 Rust 认领。
- 启动时执行一次 `recover_interrupted`：残留 `running → queued`、清理超时认领
  （宿主崩溃恢复；Node 不做这一步）。

## 2. 事件顺序

```
start ── recover_interrupted(now) ── poll ticker(20s)
                                     │
tick ── claim_due(now) ── task(queued, schedulable) claim_running 0→1
   │
   ├─ run_type = conversation_id 存在 ? "resume" : "init"
   ├─ 注册终态订阅（必须先于 sendPrompt）
   ├─ DispatchPort.dispatch(task, run_type, on_terminal)
   │     ├─ 失败 → release_claim(error)；attempt_count+1 ≥ max → mark_terminal(failed)
   │     └─ 成功 → mark_running(started_at, conversation_id, session_id)
   └─ on_terminal(outcome, error?)
         ├─ succeeded → mark_terminal(completed) + setTaskUnread
         ├─ failed + error 含 off-peak-ticket-expired → requeue_for_continuation
         │    （running→queued, schedulable=0；Node 轮询重取号后回升）
         └─ failed 其它 → mark_terminal(failed, failure_reason=error)
```

## 3. 认领与派发

- 轮询间隔默认 20s；`wake()` 立即跑一轮（合并并发请求，单飞 tick）。
- `claim_due` 一次返回全部可认领任务；逐条**串行**派发，单条失败不影响后续。
- `conversation_id` 存在 → `resumeTask` 后 `sendPrompt(offPeakRunType="resume")`；
  否则 `createTask(offPeakTaskId, modelSelection, mode=permissionMode 映射)` 后
  `sendPrompt(offPeakRunType="init")`。
- `inputId` 统一使用 `offPeakTaskId`；终态回调只接受 `input_id == offPeakTaskId`
  的事件（避免同会话其它 turn 的终态串写）。
- 派发成功且任务仍非终态 → `mark_running`（清 claim、写首段 `started_at`）；
  `mark_running` 返回 `None`（任务已被取消/暂停）→ 立即退订，不再结算。
- 派发失败：`release_claim(error)` 回到 queued 等待下一 tick；当
  `attempt_count + 1 >= max_dispatch_attempts`（默认 3）时落
  `mark_terminal(failed, dispatch_error)`，避免无限重试。

## 4. 终态结算

- `outcome == "succeeded"` → `mark_terminal(completed, ended_at=now)`。
- `outcome == "failed"`：
  - 错误文本包含 `OFF_PEAK_TICKET_EXPIRED_MARKER = "off-peak-ticket-expired"`
    （`packages/shared/src/off-peak-types.ts` 的稳定标记，agent 适配层嵌入）→
    `requeue_for_continuation`（保留会话与 `started_at`），**不落 failed**；票的
    重取由 Node 轮询完成（见 §1）。
  - 其它失败 → `mark_terminal(failed, failure_reason=error)`。
- 终态事件不带错误详情：生产 `DispatchPort` 在 failed 时从 task-index 的
  `lastError.message` 读取错误文本；读不到时按空串处理（落 failed）。
- 每次终态 best-effort `setTaskUnread(true)`（完成通知），失败不影响落库。
- 幂等：`mark_terminal`/`requeue_for_continuation` 都带状态守卫；重复终态、取消后
  迟到的终态均为 no-op；首次终态即退订并从在途表移除。

## 5. 取消与停机

- `cancelTask` 仍由管理面（Node）执行：落 `cancelled` + `stopRunningTask`；Rust 迟到
  的终态被状态守卫拒绝。
- `stop()`：标记 disposed、中止轮询、退订全部在途订阅，并对仍在途的认领执行
  `release_claim`（已 `mark_running` 的行 claim 已清，调用无副作用）。
- 运行中任务不因宿主停机回滚状态；下次 `start` 的 `recover_interrupted` 统一回收。

## 6. 端口契约

```rust
pub enum OffPeakRunType { Init, Resume } // "init" | "resume"
pub struct PortDispatchRequest { pub task: OffPeakTask, pub run_type: OffPeakRunType }
pub struct PortDispatchResult { pub conversation_id: String, pub session_id: Option<String> }
pub struct OffPeakTerminal { pub task_id: String, pub input_id: String,
                             pub outcome: String, pub error: Option<String> }
pub trait OffPeakDispatchPort {
    fn dispatch(&self, request, on_terminal: Arc<dyn Fn(OffPeakTerminal) + Send + Sync>)
        -> Future<Result<(PortDispatchResult, Unsubscribe), String>>;
    fn mark_task_unread(&self, task: &OffPeakTask) {} // 默认 no-op
}
```

生产实现（`host-channels::off_peak_host`）只经 mode-task 频道公开 call 面编排，
不复制第二条索引写入路径；终态订阅先于 `sendPrompt`。

## 7. 已知缺口

1. `filesChanged` 不回填（终态事件不携带 diff；off-peak 列表暂不显示变更数）。
2. 取号/批量轮询/管理频道仍由 Node 提供；本切片只恢复执行，不迁 `off-peak-task` 频道。
3. `offPeakRunType` 只有 `init`/`resume` 两值（协议既有约束），没有独立的重试类型。
4. 错误分类只识别 ticket-expired 标记；其余失败统一落 `failed`，不区分可重试网络错误。
5. **远端 workspace 暂不支持**：派发端口只经本地 `AgentControlClient`；带远端
   `workspaceIdentity` 的任务会派发失败并按重试上限落 `failed`。远端分派需要接入
   remote registry（16.4），不在本切片。

## 8. 验收

- `cargo test -p off-peak-runtime`：认领→派发→mark_running→终态结算、派发失败重试
  与上限、ticket-expired 回队、启动 recovery、stop 释放认领与退订。
- `cargo test -p host-channels --lib off_peak_host`：init/resume 参数映射、
  permissionMode→mode、failed 时从 lastError 取错误文本。
- `cargo test -p host-channels --lib prompt_denylist`：`sendPrompt` 透传 `offPeakTaskId`/
  `offPeakRunType` 与 OffPeakCreate denylist；`createTask(offPeakTaskId)` 使用
  deferred persistence。
