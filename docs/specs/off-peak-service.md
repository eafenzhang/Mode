# Spec：闲时任务本地生命周期（Rust `off-peak-service`）

状态：2026-11-02 第二版（全部服务方法；HTTP 客户端与定时器由调用方实现）。
真源：`packages/services/src/session/offPeakTaskService.ts`。

## 1. 范围

| Rust 方法                          | TS 方法                  |
| ---------------------------------- | ------------------------ |
| `list` / `get`                     | `list` / `get`（含模型诊断投影） |
| `pause_task`                       | `pauseTask`              |
| `delete_history`                   | `deleteHistory`          |
| `cancel_task`                      | `cancelTask`             |
| `delete_task`                      | `deleteTask`             |
| `update_task`                      | `updateTask`             |
| `validate_dispatch_model_selection` | 同名方法                 |
| `is_off_peak_terminal_status`      | `isOffPeakTerminalStatus` |
| `get_coding_plan_support` / `get_take_number_availability` | 同名方法 |
| `create_task`                      | `createTask`             |
| `continue_task`                    | `continueTask`           |
| `handle_ticket_expired_during_run` | 同名方法                 |
| `run_sync_cycle` / `start_sync` / `stop_sync` / `dispose_all` | 同名方法 |

`OffPeakServerClient` 由调用方实现（HTTP/凭证/重试）；定时器由调用方根据
`run_sync_cycle` 返回值调度。

## 2. 语义

- `list`/`get`：读取后投影模型可用性——有 modelSelection 且非终态、解析失败时置
  `schedulable=false` 并写 `modelSelectionIssue {code:"repair-required", legacyModelId,
  legacyReasoningLevel?}`；缺失 modelSelection 时也置 `schedulable=false`；终态不再检查。
- `pauseTask`：仅 `queued` 可暂停；成功回调 `onTasksChanged`。
- `deleteHistory`：只写 `historyDeletedAt`（未开始任务不写），返回最新任务。
- `cancelTask`：非终态 → 落 `cancelled`；running 且有 conversationId 时回调
  `stopRunningTask`（失败忽略）；随后 `onTasksChanged` + `settleTerminal`。
  已终态直接返回原任务；不存在返回 null。
- `deleteTask`：非终态先 cancel（含 stop/settle），再删行并广播。
- `updateTask`：仅 queued/paused；`modelSelection:null` 拒绝；候选解析失败拒绝；
  成功后广播。`modelSelection` 缺省时保留当前选择并重新解析。
- `validateDispatchModelSelection`：解析成功且 providerId/modelId 与输入一致。

## 3. 依赖注入

`OffPeakTaskEnvironment` 提供 `resolve_model_selection`、`stop_running_task`（默认 no-op）、
`settle_terminal`（默认 no-op，真实实现应写核销 outbox）与 `on_tasks_changed`。
时钟由服务构造时注入；repo 仍由调用方持有。

## 3.1 取号/轮询/核销

- `create_task`：校验 → 模型解析 → 绑定会话冲突预检 → 取号 → 落库（同 task_id 可重取）
  → 写入 nextPollAt → 广播/唤醒；失败按 client_validation / ticket_request(3101/3103/
  network/invalid_response) / local_persist 分类，不跨层传 raw error。
- `continue_task`：恢复 paused → 查票（按 ticketId 匹配）；票活则写回 schedulable/位次，
  查询失败按票活处理；票废则同 task_id 重取号。
- `handle_ticket_expired_during_run`：running → queued 后重取号。
- `run_sync_cycle`：先补报 settle outbox → 给无票 queued 补取号 → 批量状态（ready/queued/
  active/expired）→ 唤醒/广播；失败指数退避，返回钳制后的下次延迟。
- `settle_one`：无票直接 settled；有票成功或 4xx 视为 ack；网络/5xx 保持未核销。

## 4. 已知差异

1. 回调/客户端均为同步接口；Host 的 async 依赖由频道适配层包一层 `spawn_blocking` 或内部运行时。
2. 定时器、HTTP 客户端、凭证与 mock 网关仍由调用方实现；本 crate 只返回下一次延迟。
3. `run_sync_cycle` 的返回值是建议延迟；TS 的 setTimeout 重排由调用方复刻。

## 5. 验收

- `pnpm off-peak-service:fixtures` → `crates/off-peak-service/fixtures/off-peak-service.json`
  （6 条序列：列表诊断、pause/history、cancel（running/终态/缺失）、delete、update
  六类拒绝与成功、validate）。
- `pnpm off-peak-service-remote:fixtures` → `off-peak-service-remote.json`
  （8 条：create queued/ready/四类票错误/校验/绑定冲突、continue 三种票态、
  sync+settle outbox、3102 续跑与 availability/support）。
- `cargo test -p off-peak-service`（本地 6 条 + 远程 8 条 TS 对拍 + 6 条 Rust 单元测试；
  真实临时 SQLite + 桩客户端；固定时钟）。
- `cargo clippy -p off-peak-service --all-targets -- -D warnings`。
