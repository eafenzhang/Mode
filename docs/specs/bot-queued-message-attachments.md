# Bot 排队消息可携带附件（入队时物化）

状态：草案（2026-10-16）。关联：[bot-state-ownership.md](./bot-state-ownership.md)（队列写盘纪律）、
[bot-workspace-turn-admission.md](./bot-workspace-turn-admission.md)（该文规则 2 提到"本对话排队消息等终态"，
但其非目标明确不改排队语义，故本行为单独成文）、[im-bot-group-chat-alignment.md](./im-bot-group-chat-alignment.md)（G1 排队可见）。

## 背景

任务运行中收到带附件的入站消息时，现状是**直接拒绝**（`botsService.ts` 的排队分支：
有附件就回 `taskRunning`，不入队），理由是附件下载链接是**瞬时**的，延迟重投递时已经失效。

但附件字节其实**早就会落到本地**：`cacheResolvedAttachment` 把解析后的字节写到
按 `botId + providerMessageId + 附件` 生成的缓存路径，`resolveAttachmentBytes` 也会
**优先读 `localPath`**（不再回网络）。也就是说"链接会过期"只对 `downloadUrl` 成立，
对已经物化的本地文件不成立。

因此可以把这个"直接拒绝"改成"入队前先物化"：用户在任务跑动期间发的截图不再白丢。

## 产品规则

### R1 入队前物化，队列项只存本地路径

带附件的入站消息在入队前先解析并写入本地缓存；队列项只记展示元数据与本地路径：

```ts
{ kind, filename, mimeType, localPath, sizeBytes? }
```

**不存 `dataBase64`**——`queuedMessages` 落在 `bot-state` 里，把 base64 写进去会把状态文件撑到
MB 级，并让每次锁内 RMW 都背上这份体积。

### R2 预算与降级（不新增失败类别）

- 单条消息既有上限不变：最多 4 个附件、单个 5MB（越界即视为物化失败）；
- 新增**单对话队列预算**：队列内已物化附件总字节 ≤ 20MB；
- 超预算、或任一附件物化失败 → **回退到今天的行为**（回 `taskRunning` 提示、不入队），
  不新增"部分入队"这种中间态。

### R3 补投走本地路径

终态出队时用 `localPath` 读回字节（`resolveAttachmentBytes` 的第一跳），
不再访问 `downloadUrl`。附件在补投时仍作为 prompt 附件与文件行进入回合，与即时消息同一路径。

### R4 去重规则随之收紧

带附件的入队**不做文本去重**：纯附件消息的 `text` 可能为空，按文本去重既无意义，
还会把"两张不同截图、同样空文本"误判成平台重试。文本去重只对纯文本消息生效（既有窗口 60s 不变）。

### R5 状态所有者与队列上限不变

- 仍写 `bot-state` 的 `queuedMessages`，经 `BotsRepo.mutateState` 锁内原子 RMW；
  新字段只是槽内数据，不引入第二份状态；
- 队列上限 20 条、满则丢最旧的语义不变。

## 状态所有者与时序

```text
任务运行中收到消息
   │
   ├─ 无附件 ──▶ enqueueQueuedMessage（文本去重）
   │
   └─ 有附件 ──▶ 物化（下载 → 5MB 校验 → 写本地缓存）
                   ├─ 失败 / 超预算 ──▶ 回 taskRunning 提示，不入队（= 今天的行为）
                   └─ 成功 ──▶ 队列项携带 {kind, filename, mimeType, localPath}
                                  │
任务终态 ──▶ drainQueuedMessages ──▶ 出队（锁内）──▶ handleMessage(带附件)
                                  └─▶ resolveAttachmentBytes 读 localPath（不回网络）
```

## 接口

- `BotQueuedMessage` 增加 `attachments?: BotQueuedAttachment[]`（`packages/shared/src/bots.ts`，协议面）；
- 同步登记进 `queuedMessages` 的 zod schema（`.strict()`，否则读写直接抛 ZodError）；
- `enqueueQueuedMessage` 的输入增加变体（`packages/services/src/bots/messageQueue.ts`）；
- 服务层新增物化函数并接进排队分支与 drain 分支（`botsService.ts`）。

## 验收场景

1. 单测（`packages/services/test/bot-message-queue.test.ts` 扩展）：
   - 携带附件的入队项保留 `attachments`，纯文本入队项不带该字段；
   - 带附件的入队在文本相同的情况下**不**被去重；
   - 预算计算只统计附件字节，超过 20MB 时调用方应拒绝（由服务层断言预算判定）。
2. 源码契约：排队分支不再"有附件即拒绝"，且物化产物只写 `localPath` 相关字段
   （队列项里不得出现 `dataBase64`）。
3. 回归：`pnpm --filter @mode/services test`、`pnpm typecheck`、`pnpm lint`、
   `pnpm architecture:check --changed` 全绿。
4. **待实机**：任务跑动期间在企微/微信发一张截图 → 收到"已排队第 N 位" → 回合结束后该截图
   被当作附件处理（而不是提示"稍后重发"）。

## 非目标

- 不改队列上限、丢最旧语义与跨对话准入（`bot-workspace-turn-admission.md`）；
- **不做附件缓存的清理策略**（既有 `cacheResolvedAttachment` 本就长期保留文件，属存量问题，
  另立项；本 spec 不扩大也不修复它——只是新增的入队路径复用同一目录）；
- 不改即时（非排队）消息的附件处理链路。
