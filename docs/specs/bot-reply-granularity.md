# Bot 回复粒度（replyMode）默认值与选择

状态：已批准（2026-10-12）；2026-10-16 补充「逐块刷出」规则（见文末）。关联：
[wecom-reply-feedback.md](./wecom-reply-feedback.md)（企微占位流与流式接管细节）、
[im-typing-feedback.md](./im-typing-feedback.md)（等待期反馈）、
[im-bot-group-chat-alignment.md](./im-bot-group-chat-alignment.md)（群聊「全部消息」模式的沉默回执）。

## 产品规则

- 四档粒度（`BotReplyGranularity`）：`assistant_changes` 标准回复（离散气泡）、
  `assistant_toolcalls_changes` 完整回复、`summary_changes` 摘要回复、
  `streaming_card` 流式卡片（同一条消息原地增长的打字机，企微经 `replyStream` 原生支持）。
- provider 支持矩阵（`getSupportedBotReplyGranularities`）：飞书系（feishu/lark）只支持
  `streaming_card`；wecom/telegram/dingtalk 支持全部四档；微信（iLink）无消息编辑/流式通道，
  不支持 `streaming_card`；其余 provider 过滤掉 `streaming_card`。
- **默认值按 provider 区分（`getDefaultBotReplyGranularity`，`normalizeBotReplyGranularity`
  在 replyMode 未设置时取它）**：
  - **企微 wecom：`streaming_card`（2026-10-12 起）**——对齐 MyAgents 企微 PC 端
    「单条消息持续增长」的打字机观感（用户决策；配合思考期占位流的原生 loading 动态，
    全程观感与参照产品一致）；
  - 其余 provider：`assistant_changes`（全局默认 `DEFAULT_BOT_REPLY_GRANULARITY`）；
  - 飞书系因仅支持一档，实际恒为 `streaming_card`（与默认值无关）。
- **显式选择优先**：`/reply` 命令（reply.set）或设置页写入的 `replyMode` 覆盖默认；
  只有 `replyMode` 未设置时才落 provider 默认。无 provider 上下文的展示/构造兜底
  （`formatReplyGranularityLabel` 无参分支等）仍回落全局默认。
- 心跳回合强制 `summary_changes`，不随默认变化。
- **不再做读取期默认值迁移**（2026-10-20 修复）：此前 `botsConfigFileSchema` 会把企微的
  `assistant_changes` 在每次读取时翻成 `streaming_card`，导致用户在设置页切到「标准回复」
  后又被读回「流式回复」，表现为无法切换。现在显式 `replyMode` 原样保留；
  默认值只在字段缺省时由 `getDefaultBotReplyGranularity` 提供。

## 验收场景

1. 单测（`bot-reply-granularity.test.ts`）：`normalizeBotReplyGranularity("wecom", undefined)`
   为 `streaming_card`；telegram/dingtalk/weixin 未设置为 `assistant_changes`；lark 为
   `streaming_card`；显式值（含企微显式 `assistant_changes`）优先于默认。
2. schema 读取：企微显式 `assistant_changes` / `streaming_card` / `summary_changes`
   都保持原值，不被读取期迁移改写；其它 provider 同值不动。
3. 新建 bot（BotsDialog）按 provider 默认播种 `replyMode`。
4. `packages/services` 全量测试、`pnpm typecheck`、`pnpm lint`、
   `pnpm architecture:check --changed` 全绿。

## 本轮验证状态

- 已验证：见验收 1/2/4 的实际执行结果（随实现提交记录）。
- **待实机**：企微真实对话确认默认走流式卡片——同一条消息随回复增长、终稿 finish 收口；
  `/reply` 切回标准回复后恢复离散气泡。

## 变更摘要区分新增/修改/删除（2026-10-16 补充）

**背景**：交付内容里的变更摘要只有文件数 + 行数增删，新建的文件和改过的文件在 IM 回复里
长得一模一样（`ModeTaskChangedFileSummary` 只有 `added` / `removed` / `writeCount`）。

**规则**：每行文件前加上变更类型标签（`新增` / `修改` / `删除`，英文 Added / Modified / Deleted），
判定口径：

- `beforeContent === null` → **新增**；
- 最终内容为空且原本有内容 → **删除**；
- 其余 → **修改**；
- 同一文件同轮多次写入时，取首个快照的 `beforeContent` 与末个快照的 `afterContent`（与
  `buildPerTurnChangeSummaries` 的聚合口径一致）。

**实现位置（刻意选择）**：判定在 **bot 层本地推导**（`packages/services/src/bots/botChangeSummary.ts`），
`ModeTaskChangedFileSummary` **不加字段**——它是 v4 传输面的类型（`mode-protocol-v4` +
`validation` + UI 侧另有一份同名推导），加字段要同步四处 schema 与两份生产者，
而 bot 已经持有会话文件的原始快照，本地推导即可。**不要**为了这个展示需求去扩共享协议类型。

**验收场景**

1. 单测（`bot-change-summary-kinds.test.ts`）：三种类型判定、同轮多次写入的聚合、
   只取目标轮次、渲染标签（中英）与"无类型时退回原格式"。
2. 覆盖范围：正文（`assistant_changes` / `assistant_toolcalls_changes`）与流式卡片终稿同源，
   两条路径都传同一份类型表。
3. 回归：`pnpm --filter @mode/services test`、`pnpm typecheck`、`pnpm lint`、
   `pnpm architecture:check --changed` 全绿。
4. **待实机**：一次包含"新建文件 + 改文件 + 删文件"的任务，IM 回复的变更摘要里三种标签都能看到。

## 逐块刷出（2026-10-16 补充）

### 背景

`assistant_changes`（标准回复）与 `assistant_toolcalls_changes`（完整回复）两档走的是**离散气泡**，
但正文此前只在工具事件或回合末成块发出：`extractBotAssistantResponseMessages(buffer, false)`
恒返回空，于是**纯文本回合整段在回合末一次性冒出**，长回答期间 IM 里只有 typing 票在动。
企微/飞书走 `streaming_card`，不受此影响；其余通道的默认档正是受影响的那一类。

### R1 逐块刷出（气泡两档）

回合进行中，气泡档按「完整片段」刷出，不再一律等回合末：

- **片段边界**取强边界：换行，或句末标点 `。！？.!?；;`。
- 切出的前缀不足 **20 字符**（按码点，含 emoji）时不单独成气泡，继续攒——
  避免出现「好的。」这种一句话一个气泡的碎片化。
- 单段超过 **600 字符**仍无强边界时，退到弱边界（`，、：,`）或空白处切一刀，
  保证长段落也有进度；**纯 CJK 且无空白无标点**时与今天一致（继续等回合末），
  不硬切汉字。
- 片段内仍超长时，由既有 `splitLongReplyText` 按长度上限继续分片。
- 发送仍走 per-bot FIFO 队列（`outboundDelivery.ts` 的最小间隔不变），流事件本身已按任务串行消费。

### R2 例外：群聊「全部消息」模式不逐块刷出

该模式（`groupSilenceCapable`）靠**整段回复是否只有 `<NO_REPLY>`** 决定静默，
中途发言会在判定之前就把内容落到群里，破坏「沉默时完全无痕」的既有契约。
因此它的回合维持原行为：不建流式卡片、不发中途工具摘要、正文在回合末一次落定。
群聊 `mention` 模式与私聊不受此例外影响。

### R3 不受影响的部分

- `summary_changes`、`streaming_card` 两档的发送路径与时机不变；
- **流式卡片全程只含助手正文 + 变更摘要**：工具调用与思考不作为可见内容，
  进行中也不再输出「工具摘要」块（2026-10-20 用户口径，对齐 MyAgents 默认颗粒度）。
  需要逐条查看工具调用时，显式选择 `assistant_toolcalls_changes`（完整回复）。

### 与既有「非目标」的关系

[bot-outbound-target.md](./bot-outbound-target.md) 非目标里的「不改分片」是**那一轮**的边界
（指卡片渲染构成与分片）；本轮改的是气泡档的**发送时机与边界**，卡片的渲染构成、
占位流接管与出站目标均未改动。该条按本条更新理解，不另开 spec。

### 2026-10-20 工具摘要下线（流式卡片）

**背景**：用户反馈企微默认回复（`streaming_card`）在工具密集回合会刷出一长串工具摘要，
与 MyAgents 的默认回复观感不一致。

**规则**：
- `streaming_card` 的卡片内容只来自助手正文与文件变更摘要；`buildStreamingCardBlocks`
  不再消费工具块，`tool_call`/`tool_call_update` 也不再向卡片追加工具。
- 「纯工具回合」仍按原逻辑兜底：进行中显示「正在处理」，终稿无正文时显示完成提示。
- `assistant_toolcalls_changes`（完整回复）保持原语义：工具调用以独立消息显式发送；
  这是用户主动选择的完整模式，不属于默认颗粒度。
- 群聊「全部消息」的沉默契约不变：不建卡片、不中途发言。

**验收**：
- `bot-streaming-edit.test.ts` 锁定流式卡片构建不含 `type: "tools"`、服务层不再调用
  `appendStreamingCardTool`；纯工具回合的进行中/终稿兜底文案仍是「正在处理」/完成提示。
- 实机：企微默认档跑一个多工具任务，卡片只增长助手正文，不出现工具摘要列表；
  切到「完整回复」后再看到工具调用消息。

### 验收场景

1. 单测（`bot-reply-segments.test.ts`）：只有弱边界或内容不足 20 字符时不刷出；
   出现句末标点且前缀够长时按边界切成一次性片段、余量留在 buffer；
   超长无强边界段可被 600 字符兜底切开且不丢字符；纯 CJK 无边界时不切。
2. 源码契约（`bot-reply-segments.test.ts`）：`flushAssistantReplyBuffer` 的非强制分支
   在 `groupSilenceCapable` 回合不得走逐块刷出（否则 `<NO_REPLY>` 之前已有气泡落地）。
3. 回归：`pnpm --filter @mode/services test`、`pnpm typecheck`、`pnpm lint`、
   `pnpm architecture:check --changed` 全绿。
4. **待实机**：微信 / 钉钉私聊发一个多段回答，正文应随生成分次到达，而不是回合末整段落下；
   群里（`always` 模式）仍应保持沉默无痕。

## 流式不可用时回退逐块气泡（2026-10-21 补充）

### 背景

实机反馈：企微机器人在真实对话里**桌面回合跑完才收到整段回复**，过程中看不到正文增长，
与 MyAgents「同一条消息持续增长」的观感不一致。

根因在「流式卡片不可用」时的降级路径：

- 企微流式回复必须透传入站帧的 `req_id`（`replyStream(frame, …)`）。**桌面端发起的回合**
  没有入站帧；入站帧过期/不可寻址时 `replyStream` 也会失败。
- `wecomProvider.createStreamingReplyCard` 在无帧时仍返回一个句柄，并把该 `streamId`
  标记为「仅终稿推送」：进行中的 `updateStreamingReplyCard` **直接 return**，直到终稿才
  `pushMarkdownChunks` 整段推送。服务层因此认为卡片可用，继续把正文留在缓冲区——
  用户侧表现为「回合结束才回复」。
- 卡片创建连续失败触发熔断后，`syncStreamingCardReply` 对终稿也直接返回；而终态分支
  只走卡片路径并 `return`，**终稿回复可能整体丢失**。

### 产品规则

- **流式能力按回合目标判定，不只按 provider**：`BotProviderAdapter` 新增可选
  `canWriteStreamingReply(bot, target)`。企微在无连接/无该会话入站帧时返回 `false`；
  服务层据此从第一个增量起就走离散气泡，不再等终稿。
- **流式卡片不可用时，正文按离散档规则逐块刷出**：卡片路径仍累积助手正文；
  每次卡片投递成功后丢弃已交付前缀。一旦判定不可用（能力探测为 false、创建返回
  `null`、或熔断打开），立即把未交付内容交给 `flushAssistantReplyBuffer(false)`，
  不把正文攒到回合结束。
- **终稿必须有落点**：终态若卡片不可用，走气泡路径发送剩余正文与变更摘要；
  不允许因为「以为卡片可用」而丢弃终稿。
- **企微 provider 不再提供「仅终稿推送」模式**：
  - `createStreamingReplyCard` 无连接/无帧时返回 `null`（服务层按可重试失败处理）；
  - 首次 `replyStream` 失败时直接抛错（重试/熔断后由服务层回退气泡）；
  - 不再把 `streamId` 登记为 `null` 帧后静默吞掉中间帧。
- 首帧门槛（20 字符/句末标点/工具事件）与既有逐块刷出边界不变；本轮只修
  「不可用时憋到终稿」和「终稿丢失」。

### 状态所有者与时序

流式可用性状态（`streamingCardUnavailable`）归 `watchTaskStream` 的单次回合闭包所有，
不落盘、不进协议；provider 只回答「此刻能否写流」，不持有服务层状态。

```text
agent_message_chunk
  ├─ 卡片路径（canWrite=true）
  │    ├─ append 正文 → sync
  │    ├─ 成功 → 丢弃已交付前缀
  │    └─ 不可用 → 置 unavailable → flush 气泡（逐块）
  └─ 气泡路径（canWrite=false / 非卡片档）
       └─ flushAssistantReplyBuffer(false)

task_complete
  ├─ 卡片可用 → 终稿卡片（正文 + 变更摘要）
  └─ 卡片不可用 → flush(force) 剩余正文 + 变更摘要气泡
```

### 验收场景

1. 单测（`bot-wecom-typing.test.ts` 或新增工具用例）：企微 `canWriteStreamingReply`
   无帧为 `false`、有帧为 `true`；`createStreamingReplyCard` 无帧返回 `null`；
   首帧 `replyStream` 失败时抛出而不是返回降级句柄。
2. 源码契约（`bot-reply-segments.test.ts`）：`flushAssistantReplyBuffer` 的卡片早退
   必须带 `!streamingCardUnavailable`；终态分支在卡片不可用时必须落到气泡路径
   （`flushAssistantReplyBuffer(true)` + 变更摘要）；provider 源码不再出现
   `streamFrames.set(streamId, null)` 的「仅终稿推送」路径。
3. 回归：`pnpm --filter @mode/services test`、`pnpm typecheck`、`pnpm lint`、
   `pnpm architecture:check --changed` 全绿。
4. **待实机**：企微私聊跑一个多工具任务，过程中正文随生成出现在同一条/多条消息里；
   桌面端发起的绑定会话回合也能在 IM 侧看到分次回复，而不是回合末整段出现。
