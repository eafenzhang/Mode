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
- **存量迁移**（`botsConfigFileSchema` 的 preprocess，幂等）：provider=wecom 且
  `replyMode` 为旧默认 `assistant_changes` 的记录，读取时翻转为 `streaming_card`；
  其它 provider 与企微上显式选过的其它粒度保持原样。迁移无法区分「当年沿用默认」与
  「当年显式点过标准回复」——接受该歧义：确实想要气泡的企微用户可用 `/reply` 改回并持久生效。

## 验收场景

1. 单测（`bot-reply-granularity.test.ts`）：`normalizeBotReplyGranularity("wecom", undefined)`
   为 `streaming_card`；telegram/dingtalk/weixin 未设置为 `assistant_changes`；lark 为
   `streaming_card`；显式值（含企微显式 `assistant_changes`）优先于默认。
2. schema 迁移：企微 `assistant_changes` → `streaming_card`；telegram 同值不动；
   企微显式其它粒度不动；重复 parse 幂等。
3. 新建 bot（BotsDialog）按 provider 默认播种 `replyMode`。
4. `packages/services` 全量测试、`pnpm typecheck`、`pnpm lint`、
   `pnpm architecture:check --changed` 全绿。

## 本轮验证状态

- 已验证：见验收 1/2/4 的实际执行结果（随实现提交记录）。
- **待实机**：企微真实对话确认默认走流式卡片——同一条消息随回复增长、终稿 finish 收口；
  `/reply` 切回标准回复后恢复离散气泡。

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
- **终稿构成不变**：仍只含助手正文 + 变更摘要，工具与思考不进交付内容。

### 与既有「非目标」的关系

[bot-outbound-target.md](./bot-outbound-target.md) 非目标里的「不改分片」是**那一轮**的边界
（指卡片渲染构成与分片）；本轮改的是气泡档的**发送时机与边界**，卡片的渲染构成、
占位流接管与出站目标均未改动。该条按本条更新理解，不另开 spec。

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
