# Bots 服务拆分（评估与方案）

状态：草案（2026-10-16，**本轮只出方案、未执行**）。关联：
[bot-state-ownership.md](./bot-state-ownership.md)（拆分不得引入第二份状态）、
[bot-queued-message-attachments.md](./bot-queued-message-attachments.md)、
[im-bot-group-chat-alignment.md](./im-bot-group-chat-alignment.md)。

## 现状

`packages/services/src/bots/botsService.ts` **8729 行**，靠文件头一行
`/* eslint-disable max-lines -- Bots 服务仍复用原 RPC 文件名，先把鉴权、命令路由、
Mode Agent 桥接收口在同一服务内 */` 压着。全部逻辑在一个 `createBotsService` 闭包里，
闭包内直接共享 30+ 个 `Map`/`Set`（stream 订阅、typing 目标、运行中任务、待选交互、
发送队列、去重表、缓存表…）。

代价是真实的：改任何一处都要先通读闭包，且"哪些状态是进程内可丢、哪些必须落盘"
只散落在注释里，没有结构去承载。

## 为什么本轮不执行

纯搬移（无行为变化）也需要逐块提交 + 每块回归：这类改动最容易在"看起来只是换个位置"
的地方引入闭包捕获顺序、初始化时序（例如 `providers` 与 runtime 的创建顺序有显式注释约束）
与状态可见性变化。它是 3–5 个 PR 的量，放在功能线（群聊 G3/G4/G7、通道探针）还没收敛时做，
会持续与功能改动冲突。**先出方案，等群聊与探针那批落地后再排。**

**2026-10-16 追加的判断**：约束 0（来源判定依赖文件路径）让"现在开始拆"的性价比更低——
在 clean-room 这条线还需要逐文件判来源的时候拆分，会削弱判定能力。
本轮只落了三个**基线里不存在**的新文件（`botText.ts` 早有、`groupHistoryStore.ts`、
`botChangeSummary.ts`），它们是按约束 0 的安全做法拆出来的，可继续沿用这个模式小步走。

## 拆分约束（先定死，避免拆出问题）

0. **clean-room 来源判定依赖文件路径**（2026-10-16 补充，**这条是"先别拆"的实质理由**）：
   判断某个文件是否属上游衍生实现，用的是 `git cat-file -e upstream/zcode-baseline:<path>`
   + 逐字节比对——**按路径**比对。把 `botsService.ts`（属"基线存在但已改动"）里的代码搬到新文件，
   会让新文件里的那部分**不再能被路径比对识别为上游来源**，
   也让 `botsService.ts` 的"已改动面"变得难以判定。也就是说：拆分不只是重构，
   它会**削弱 clean-room 的判定能力**。
   → 结论：拆分要么等到上游同步线彻底收口（`upstream-tracking-audit-only` 那条线不再需要
   逐文件来源判定），要么**只搬能证明是自研的代码**（例如纯新增的 `botText.ts` /
   `groupHistoryStore.ts` / `botChangeSummary.ts` 这类基线里不存在的文件），
   不要成片搬运 `botsService.ts` 的内容。
1. **不改行为**：只做搬移 + 显式依赖注入；每个 PR 的验收是"services 全量测试 0 失败 +
   typecheck + lint + architecture:check 全绿"，不允许顺带改语义。
2. **不换 RPC 文件名/注册名**：`bots` 这个服务名与文件位置是既有兼容面，拆分是"内部模块化"，
   不是"重命名服务"。
3. **状态仍然单例**：那些进程内 `Map`/`Set` 是"进程内可丢"状态（`bot-state-ownership.md`
   规则 2 声明过不做进程内缓存），必须由单一构造点持有并显式注入，
   **不得**拆成各自模块的模块级单例。
4. **初始化顺序不变**：`providers` → runtime（runtime 复用 provider 的企微连接注册表）
   这一顺序在注释里有明确约束，拆分后由构造点显式串联，不能靠 import 顺序。

## 建议的边界（按职责切五块，从外到内）

| # | 模块（拟） | 搬移内容 | 依赖方向 |
| --- | --- | --- | --- |
| 1 | `botAccess.ts` | `withAuthorizedContext`、`findAuthorizedBot`、`findBoundUser`、`isUserCommandAllowed`、私聊/群聊双闸门 | 只依赖 repo/config，不依赖 task 桥接 |
| 2 | `botCommandRouter.ts` | `commandParser` 的消费、命令回执、`/bind` `/项目` `/模型` `/reply` 等分支 | 依赖 1 与 5 |
| 3 | `botConversationContext.ts` | `readContext` / `writeContext` / `queueContextMessage` / `drainQueuedMessages` / 工作区纠正 | 依赖 repo + 群历史 |
| 4 | `botOutbound.ts` | `sendOutbound` / `deliverWithRecovery` / `parkOutboundMessage` / `flushPendingDeliveries`（`outboundDelivery.ts` 已拆出纯函数层，这里搬剩下的 IO 层） | 依赖 provider 适配器 |
| 5 | `botSessionBridge.ts` | `handleMessage` 主体、`createTask` 起跑、`watchTaskStream`、reconcile 触发 | 依赖 1–4 |

`botsService.ts` 最终只剩：构造依赖、装配 1–5、暴露 `IBotsService` 方法、dispose。

## 验收场景（每个 PR 都适用）

1. `pnpm --filter @mode/services test` 全绿（当前 361 项、0 失败为基线）；
2. `pnpm typecheck`、`pnpm lint`（0 error）、`pnpm architecture:check --changed`（0 违规）；
3. 源代码契约：拆分后 `botsService.ts` 行数下降且**不再出现 `eslint-disable max-lines`**；
4. 行为不变量（人工评审 + 现有契约测试）：`resolveOutboundTarget` 仍是唯一出站目标函数、
   `BotsRepo.mutateState` 仍是唯一状态写入入口、runtime 创建顺序未变。

## 非目标

- 不改任何行为、文案、协议字段；
- 不把进程内状态改成持久化（也不引入第二份缓存）；
- 不动 `IBotsService` 的方法签名与注册名；
- 不重排 `providers/` 与 `*ChannelRuntime.ts`（它们的边界已经清楚）。
