# Bot 同 workspace 回合并入上限（并发准入门）

状态：草拟（2026-10-10，随 B→A→C 批准的拆分执行）。
关联：[bot-outbound-delivery.md](./bot-outbound-delivery.md)、[bot-state-ownership.md](./bot-state-ownership.md)。

## 背景

多个用户同时首次触发 bot 时，每个对话各自冷启动一个 agent 回合（`createTask` →
`sendPromptInBackground`），N 个回合在**同一 workspace** 内同时起跑：CPU/内存尖峰、
模型侧并发限流、同目录命令互踩。现状没有任何准入控制——`runningTasks` 只用于
「本对话是否在跑」的排队判断，不约束跨对话总数。

## 产品规则

1. **上限**：同一 workspace 内由 bot 侧发起的**运行中回合**数 ≤ 2
   （`BOT_WORKSPACE_TURN_CONCURRENCY_CAP = 2`）。
2. **排队语义**：超出上限的回合在入队点**静默等待**（不发提示），直到有回合进入终态
   释放槽位后按等待顺序起跑。等待无超时——与既有「本对话排队消息等终态」的语义一致
   （若 `runningTasks` 泄漏，今天的排队队列同样会等不到终态；本门不引入新的失败类别，
   自愈仍依赖既有 staleness 修复，见 `isContextActiveTaskRunning` 的 runningTasks 对账）。
3. **计数口径**：槽位在 `runningTasks.add` 之前获取、在对应的
   `runningTasks.delete`（终态 5483 / 发送失败 6338 / staleness 对账 6733 /
   生命周期清理 8297）与 `clear`（dispose）处释放——门的跟踪表与 runningTasks
   严格同生共死，不产生独立于现有生命周期的第二套真相。
4. **跨 workspace 独立**：每个 workspaceKey 一条计数与 FIFO 等待队列，互不阻塞。
5. **fail-open 不适用**：不做超时兜底（不能用超时掩盖同步问题）；等待方在
   `enqueueInboundProcessing` 串行链内 `await`，该对话的后续消息本来就会排队。
6. **v1 已知限制**：跨对话冷启动等待期间无「排队中」提示（命中上限才发生，低频）；
   提示文案留作后续。

## 状态所有者与时序

```text
对话A 首发/续跑 ──┐                       对话B 首发（同 workspace）
                  │                              │
                  ▼                              ▼
     turnGate.enter(workspaceKey, taskId)  enter()：count>=2 → FIFO 等待
                  │                              │
                  ▼                              │ 等待中（串行链内 await）
     runningTasks.add(taskId)  ← 终态/失败/对账/清理 delete 点
                  │                              │
        回合运行（watchTaskStream…）             │
                  │                              ▼
                  ▼ ← delete(taskId) → turnGate.exit(taskId) → 唤醒队首
     task_complete / task_error                    │
                                                   ▼
                                      enter() 通过 → runningTasks.add → 起跑
```

- **唯一所有者**：门状态（`workspaceKey → taskId 集合` + 等待队列）内存在
  `workspaceTurnGate` 模块实例（botsService 闭包内单例），随 dispose 清空；
  与 `runningTasks`（同闭包）成对维护，不落盘、不做第二份持久化真相。
- **入口**：`handleMessage` 两个 add 点之前（冷启动分支与续跑分支）。
- **出口**：全部 4 个 delete 点 + dispose 的 clear。

## 验收场景

1. 单测（`packages/services/test/bot-workspace-turn-gate.test.ts`）：
   - cap=2：第三个 enter 阻塞，exit 后按 FIFO 顺序放行；
   - 不同 workspaceKey 互不影响；重复 exit / 未知 taskId exit 为 no-op；
   - `reset()` 清空跟踪并唤醒所有等待者（等待者随后按当前计数重新竞争）。
2. 接入完整性：`rg` 断言每个 `runningTasks.delete|clear` 旁有对应 `turnGate.exit|reset`
   （由实现评审 + 既有源码断言风格测试固定，若可断言）。
3. 现有 services 全量测试全绿；`pnpm typecheck`、`pnpm lint`、
   `pnpm architecture:check --changed` 通过。

## 非目标

- 不做跨对话「排队中」用户提示（v1 已知限制）；
- 不改本对话既有的排队/入队语义（`messageQueue`、`drainQueuedMessages`）；
- 不做跨进程（多窗口）全局并发统计——各窗口独立 host、各自的 agent 进程，
  单进程内准入即为本次目标；
- 不做超时/心跳自愈新机制（沿用既有 staleness 对账）。
