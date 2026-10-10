# Bot 出站投递可靠性（微信 context 过期补投与失败可见性）

状态：草拟（2026-10-10，拆分与顺序已经用户确认：B 出站投递 → A 状态竞态 → C 并发上限）。

## 背景

实机事故（2026-10-10，`~/.mode/v2/logs/2026-10-10.log`）：21:58:34 起微信
`/ilink/bot/sendmessage` 连续 136 次返回服务端 `prepare failed`，直到 22:52 无一成功。
桌面端（任务流镜像）助手回复大量正常生成，微信端只收到 21:30–21:58 的少量回复。
已确认的根因链：

1. 推送始终携带**最后一条入站消息**（21:30:50）捕获的 `context_token`
   （`createOutbound` → `weixinProvider.send`）；服务端在约 28 分钟后拒绝准备会话，
   21:30:50 之后无新入站 → token 永不刷新 → 后续新回合首发即失败；
2. 发送失败只在 stream 事件队列 `warn` 一行后**丢弃该条回复**：无重试、无入队、
   无用户可见提示（`botsService.ts` stream event catch）；
3. 出站无 per-bot 串行/间隔，同一 bot 的发送可并发突发。

## 产品规则

1. **出站统一串行**：所有文本出站经 `sendOutbound` 进入 per-bot FIFO 队列，
   相邻发送最小间隔 300ms（仅平滑突发，不声称满足任何平台配额）。
2. **微信 provider 错误结构化分类**（错误对象携带 code + 完整 `ret/errcode/errmsg`）：
   - `context_expired`：服务端 errmsg 为 `prepare failed`（会话准备失败类）；
   - `retryable`：HTTP 5xx / 429 / 网络超时；
   - `session_expired`：HTTP 401 / 403 / token 缺失；
   - 其余（含其他 provider 抛出的普通 Error）：**行为保持现状**（沿既有 catch 路径
     冒泡/记录），不进入新恢复管线——控制爆炸半径。
3. **恢复顺序**（仅 `context_expired`）：
   ① 对话存有与本次所用不同的 `lastContextToken` → 换新 token 重试 1 次；
   ② 无新 token → 去掉 `context_token` 字段重试 1 次；
   ③ 仍失败 → 停放入该对话的 `pendingDeliveryQueue`，并尝试发送一次性提示。
4. **`retryable`**：有界退避重试 2 次（500ms、2s）；仍失败转入 3 的入队路径。
   **`session_expired`**：不重试，直接入队 + 提示（detail 记入日志，提示用户重扫码需
   人工介入，文案不做平台特化）。
5. **一次性提示**：每个失败 episode 只发一次（`deliveryNoticeAt` 时间戳标记；
   任一补投成功后清除标记）；提示本身发送失败仅记日志，不再递归入队。
6. **补投**：该对话收到新入站消息（新 token 写入 `lastContextToken`）后、
   处理该新消息**之前**，按序补投 parked 回复；**遇第一次失败即停止本轮补投**
   （服务端状态对后续条目同样不利，避免连环失败放大 attempts），
   失败条目保留重投（`attempts` 递增，超过 8 次丢弃该条并 `warn`，防止永久积压）。
7. **生命周期对齐**：`pendingDeliveryQueue` 与既有 `queuedMessages` 同步清理
   （绑定工作区 re-pin、任务切回 draft、`resetBotState`）。

## 状态所有者与时序

```text
新入站消息 ──▶ enqueueInboundProcessing[对话]（既有串行链）
                 │
                 ├─ 写 lastContextToken（token 变化时才写，避免无谓落盘）
                 ├─ 补投 pendingDeliveryQueue（新 token 先到先补，失败保留）
                 └─ 正常业务处理（既有流程）

stream/回复 ──▶ sendOutbound ──▶ per-bot FIFO（300ms 间隔）
                 │
                 ├─ 成功 ──▶ 结束（清 deliveryNoticeAt）
                 ├─ context_expired ──▶ 换新 token 重试 → 去 token 重试 → 入队+提示
                 ├─ retryable ──▶ 退避重试 ×2 → 入队+提示
                 ├─ session_expired ──▶ 入队+提示
                 └─ 其他 Error ──▶ 既有行为不变
```

- **状态唯一所有者**：`BotConversationState`（`bot-state.v3.json` 会话槽位），
  新增字段 `lastContextToken?`、`pendingDeliveryQueue?`、`deliveryNoticeAt?`；
  只有 botsService 在 per-conversation 串行链内写入，无第二写入方。
- **纯函数模块**（新增 `packages/services/src/bots/outboundDelivery.ts`）：
  错误分类映射、park/flush/attempt 裁剪、per-bot 串行队列工厂——不依赖 IO，便于单测。
- **i18n**：`packages/services/src/bots/messages.ts` 新增 `deliveryParked`（zh-CN/en-US）。

## 验收场景

1. 单测（`packages/services/test/bot-outbound-delivery.test.ts`）：
   - 分类：errmsg=`prepare failed` → `context_expired`；HTTP 500/429 → `retryable`；
     401/403 → `session_expired`；非零 ret 其他 errmsg → 不分类（保持现状语义）；
   - park/flush：三连失败入队、新 token 补投顺序、attempts 上限丢弃、
     提示每 episode 一次且补投成功后复位；
   - 串行队列：同 bot 严格 FIFO、跨 bot 互不阻塞、最小间隔生效。
2. 微信 provider 层：`requestWeixinJson` 把 `ret/errcode/errmsg` 与 HTTP 状态映射到
   结构化错误（分类函数纯测，不发真实请求）。
3. 现有 bot 测试全绿；`pnpm typecheck`、`pnpm lint`、
   `pnpm architecture:check --changed` 通过。
4. 实机回归：复现长回合后，微信端先收到一次性提示，新发一条消息后收到补投回复。

## 非目标

- 不做主动续期探测/心跳（失败提示 + 新消息补投已覆盖可见性与恢复）；
- 不改其他 provider 的失败语义与卡片/ACK 路径；
- 跨对话状态 RMW 竞态与热路径 IO 优化属子项目 A（`bot-state-ownership.md`）；
- 任务并发准入上限属子项目 C；
- 不追溯补发 2026-10-10 事故中已丢弃的历史回复。
