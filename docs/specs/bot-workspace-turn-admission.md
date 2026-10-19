# Bot 同 workspace 回合并入上限（并发准入门）

状态：已修订（2026-10-20，A+ 取消 workspace 级上限 2，改为单实例全局有界并发）。
关联：[bot-outbound-delivery.md](./bot-outbound-delivery.md)、[bot-state-ownership.md](./bot-state-ownership.md)。

## 背景

多个用户同时首次触发 bot 时，每个对话各自冷启动一个 agent 回合（`createTask` →
`sendPromptInBackground`），N 个回合在**同一 workspace** 内同时起跑：CPU/内存尖峰、
模型侧并发限流、同目录命令互踩。现状没有任何准入控制——`runningTasks` 只用于
「本对话是否在跑」的排队判断，不约束跨对话总数。

## 产品规则

1. **上限**：单服务实例内由 bot 侧发起的**运行中回合**总数 ≤
   `BOT_GLOBAL_TURN_CONCURRENCY_CAP`（默认 8）。不再按 workspace 限制 2；
   同一会话的串行仍由 `enqueueInboundProcessing` 的对话键队列保证。
2. **排队语义**：超出上限的回合在入队点等待，**首次挂起时向该回合的发起方发一次「已排队」提示**
   （见规则 6），直到有回合进入终态释放槽位后按等待顺序起跑。等待无超时——与既有「本对话排队消息等终态」的语义一致
   （若 `runningTasks` 泄漏，今天的排队队列同样会等不到终态；本门不引入新的失败类别，
   自愈仍依赖既有 staleness 修复，见 `isContextActiveTaskRunning` 的 runningTasks 对账）。
3. **计数口径**：槽位在 `runningTasks.add` 之前获取、在对应的
   `runningTasks.delete`（终态 5483 / 发送失败 6338 / staleness 对账 6733 /
   生命周期清理 8297）与 `clear`（dispose）处释放——门的跟踪表与 runningTasks
   严格同生共死，不产生独立于现有生命周期的第二套真相。
4. **跨 workspace 共享全局槽位**：不同 workspace/对话竞争同一组全局槽位；
   全局槽位未满时，同一 workspace 的不同群/私聊可以并行，不再被 workspace 级
   上限 2 卡住。
5. **fail-open 不适用**：不做超时兜底（不能用超时掩盖同步问题）；等待方在
   `enqueueInboundProcessing` 串行链内 `await`，该对话的后续消息本来就会排队。
6. **排队提示**（2026-10-14：由 v1 已知限制转为已实现）。跨对话等待期间发一次提示文案
   `turnQueued`。约束：
   - **只发一次**：`enter()` 首次因超限挂起时回调一次，同一回合的后续轮询不再重复——等待
     主要发生在 `enqueueInboundProcessing` 串行链内，重复提示只会刷屏；
   - **提示不阻塞准入**：回调是同步 fire-and-forget，发送失败只记 warn，不影响槽位获取与
     回合起跑（与上方 `deletedTaskReplaced` 通知同一处置口径）；
   - **门保持纯原语**：回调由调用方注入（`enter(workspaceKey, taskId, { onWait })`），门本身
     不依赖消息发送、不引入 IO。

## 状态所有者与时序

```text
对话A 首发/续跑 ──┐                    对话B 首发（同/异 workspace）
                  │                              │
                  ▼                              ▼
     turnGate.enter(workspaceKey, taskId)  enter()：全局在跑数>=cap → 全局 FIFO 等待
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

- **唯一所有者**：门状态（全局 `taskId → workspaceKey` 映射 + 全局 FIFO 等待队列）内存在
  `botTurnGate` 模块实例（botsService 闭包内单例），随 dispose 清空；
  与 `runningTasks`（同闭包）成对维护，不落盘、不做第二份持久化真相。
- **入口**：`handleMessage` 两个 add 点之前（冷启动分支与续跑分支）。
- **出口**：全部 4 个 delete 点 + dispose 的 clear。

## 验收场景

1. 单测（`packages/services/test/bot-workspace-turn-gate.test.ts`）：
   - cap=2：第三个 enter（任意 workspace）阻塞，exit 后按全局 FIFO 顺序放行；
   - 全局槽位未满时，同一 workspace 的多个对话可并行（不再有 workspace 级上限 2）；
   - 重复 enter 幂等；重复 exit / 未知 taskId exit 为 no-op；
   - `reset()` 清空跟踪并唤醒所有等待者（等待者随后按当前全局计数重新竞争）。
2. 接入完整性：`rg` 断言每个 `runningTasks.delete|clear` 旁有对应 `turnGate.exit|reset`
   （由实现评审 + 既有源码断言风格测试固定，若可断言）。
3. 现有 services 全量测试全绿；`pnpm typecheck`、`pnpm lint`、
   `pnpm architecture:check --changed` 通过。

## 非目标

- 不改本对话既有的排队/入队语义（`messageQueue`、`drainQueuedMessages`）；
- 不做跨进程（多窗口）全局并发统计——各窗口独立 host、各自的 agent 进程，
  单进程内准入即为本次目标；
- 不做超时/心跳自愈新机制（沿用既有 staleness 对账）。
