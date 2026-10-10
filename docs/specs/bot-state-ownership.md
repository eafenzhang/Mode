# Bot 状态持久化单所有者（锁内原子读-改-写）

状态：草拟（2026-10-10，修订版设计已经用户确认：放弃内存单所有者，采用锁内原子 RMW）。
关联：[bot-outbound-delivery.md](./bot-outbound-delivery.md)（停放补投队列也持久化在本状态文件）。

## 背景

`bot-state.v3.json` 的既有写入模式是 `readState() → 内存改 → writeState(整文件覆盖)`。
`withFileLock`（`packages/shared/src/node/privateFilePersistence.ts`）只序列化**单次读**与
**单次写**，两次锁之间不隔离，产生两类已确认的丢更新竞态：

1. **进程内跨对话**：企微/钉钉入站是 fire-and-forget 分发（`wecomChannelRuntime.ts:250`、
   `dingtalkChannelRuntime.ts:220`），不同对话并发走到 RMW——两个对话各自拿到同一快照，
   后写者覆盖前写者刚落盘的排队消息 / `activeTaskId` / 去重表 / `telegramOffset`。
2. **跨窗口多进程**：`bot-state.v3.json` 由多个 Mode 窗口共享（微信/企微轮询锁注释明确
   「每个窗口都有独立 host」「handled by another Mode window」），跨进程的
   读-改-写同样在两次锁之间失守。

同文件还存在热路径读放大（每条消息十余次全量读+zod 解析）。原「进程内内存单所有者 +
写穿透」方案被多窗口证据否决：进程内缓存对另一窗口的写入必然陈旧，补失效协议的复杂度
高于收益。

## 工程规则

1. **唯一写入命令入口**：状态文件的一切读-改-写必须经
   `BotsRepo.mutateState(mutator)`——同一把文件锁内完成「读最新（含迁移）→ 执行
   mutator → zod 校验 → 原子写盘」。禁止再出现 `readState…writeState` 快照覆盖模式；
   只读观察继续用 `readState()`。
2. **唯一事实**：文件锁内的最新状态即唯一权威；进程内不保留第二份状态缓存
   （多窗口共享，缓存需跨进程失效协议——明确不做）。
3. **mutator 不变量**：mutator 内**禁止**调用 repo 的任何文件操作（同一路径嵌套
   `withFileLock` 会因进程内 FIFO 队列自锁死锁），只允许对传入状态对象的同步修改；
   释放订阅、停 typing、广播等副作用一律在 `mutateState` 返回之后执行。
4. **槽位隔离**：对话级 mutator 只修改自己的 conversation 槽位与通道级自身字段；
   不读改其他对话的槽位内容。
5. **读路径不变**：`readContext` 等纯读逻辑与现有语义保持不变（不做缓存）。

## 状态所有者

```text
写入方：入站串行链（对话）/ 心跳 / UI 焦点 / 启动迁移 / bot 增删改 / 出站停放补投
   │  （进程内另有 per-conversation 串行链保证同对话顺序）
   ▼
BotsRepo.mutateState(mutator) ── withFileLock：进程内 FIFO + 跨进程 OS 锁
   │    锁内：readStateLocked → mutator(state) → zod parse → writeJson
   ▼
bot-state.v3.json（唯一事实）
```

**覆盖点清单**（`botsService.ts` 全部 10 处写入；行号以实现时为准）：

| 写入方 | 语义 |
|---|---|
| `patchBotChannelState` | 通道级字段（去重表/游标/心跳/激活时间） |
| `writeContext` | 对话槽位替换（副作用：撤流订阅、停 typing → 锁外执行） |
| `queueContextMessage` | 任务运行中入队 |
| `drainQueuedMessages` | 终态出队 |
| 启动 legacy 重映射固化 | v3→v4 形状固化 |
| `clearDeliveryNoticeIfSet` / `parkOutboundMessage` | 出站停放（子项目 B） |
| bot 凭据重置 / bot 删除 | `delete state.bots[botId]` |
| `resetBotState` | 按对话键或整 bot 重置 |

## 验收场景

1. **并发丢更新回归**（`packages/services/test/bot-state-mutation.test.ts`，临时数据目录
   `setDataBaseDir` 隔离）：并发 20 个 `mutateState` 各自在不同对话槽位写入自己的标记
   → 全部保留（零丢失）；同一槽位的 20 次追加全部保留。
2. **死锁防线**：mutator 同步修改、无嵌套 repo 调用——由实现审查与既有测试保证；
   若新增嵌套调用，`withFileLock` 的 FIFO 会超时暴露（`lockMaxWaitMs`）。
3. 现有 services 全量测试全绿（含子项目 B 的 9 个出站用例）。
4. `pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed` 通过。
5. 实机：微信/企微多用户并发收发后，排队消息与 `activeTaskId` 无互相覆盖
   （对照 `bot-state.v3.json` 逐对话核对）。

## 非目标

- 不做进程内状态缓存与跨进程失效协议；
- 不收口 config 文件（`bot-config.v3.json`）的 RMW——写入方少且低频，另立后续任务；
- 不做 state 与 bindings 的跨文件事务（各自文件锁内原子，不承诺跨文件一致性）；
- 不改变 per-conversation 入站串行链（`enqueueInboundProcessing`）与排队语义；
- 不引入内存单所有者/写穿透队列（多窗口证据已否决）。
