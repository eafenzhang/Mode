# Bot 回复粒度（replyMode）默认值与选择

状态：已批准（2026-10-12）。关联：[wecom-reply-feedback.md](./wecom-reply-feedback.md)（企微占位流与流式接管细节）、[im-typing-feedback.md](./im-typing-feedback.md)（等待期反馈）。

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
