# Bot 跨聊天消息交接（Agent 工具触发）

状态：草案（2026-10-23，设计已口头确认，待 spec 审阅）。
关联：[im-bot-agent-channel-aplus.md](./im-bot-agent-channel-aplus.md)、[bot-outbound-media.md](./bot-outbound-media.md)、
[bot-outbound-delivery.md](./bot-outbound-delivery.md)、[bot-workspace-turn-admission.md](./bot-workspace-turn-admission.md)。

## 背景

对齐 MyAgents 0.5.4（只对行为，clean-room 不读实现）：

- CHANGELOG「跨聊天消息交接」：Agent 可将消息、背景说明和附件交给同一 Agent
  所连接的其他聊天，由目标会话结合已有上下文处理和转达。

现状差距：我们一个 bot 已天然支持多对话（多私聊 + 多群，会话键
`private:<userId>` / `group:<chatId>`），但对话之间只能由**用户**在各自聊天里驱动；
Agent 在 A 对话的回合中无法把内容投给同一 bot 的 B 对话。

用户决策（2026-10-23）：Agent 工具触发（不做用户命令入口）。

## 产品规则

### R1 工具与暴露面

- 新增 Agent 工具 `handoff_message`，入参：
  - `target`（必填）：目标对话键，与现有 `parseBotConversationKey` 格式一致
    （`private:<userId>` / `group:<chatId>`）；
  - `message`（必填）：交接正文；
  - `context`（可选）：背景说明，随交接一并注入目标回合；
  - `attachments`（可选）：本地路径数组。
- **只在 IM 渠道发起的 bot 回合中暴露**；桌面 / 手机 Web 会话永远不注入该工具
  （对齐 0.5.4「普通桌面对话不受渠道发送指令影响」，与 A+ 投递模式 tool_only
  的工具暴露边界同源）。
- 同一 bot 的所有对话共用该 bot 的工具面；目标必须是**同一 bot** 的对话，
  跨 bot 交接不存在（目标键不在本 bot 对话列表即拒绝）。

### R2 目标列表注入

- bot 回合构造上下文时，注入该 bot 当前全部对话的键与显示名
  （私聊显示用户 id/昵称，群显示群名），作为 `handoff_message` 的可选目标清单；
- `target` 不在清单内：工具返回错误，错误文本回显可用目标清单，不投递、不建对话。

### R3 投递语义

- 交接到达目标对话后，等价于目标对话产生了一条**新的外部输入**：
  进入目标对话现有的 `messageQueue` 串行队列，按目标对话自己的回合准入执行
  （排队中 `turnQueued` 提示不发给交接方，只在工具结果里回报「已排队」；
  队满按现有上限拒绝并把原因返回给调用方 Agent）。
- 目标回合结合**目标对话自身历史**处理：`context` 作为背景注入，
  `message` 作为正文；注入块带不可信标注（复用群历史 untrusted note 模式），
  且对模型可见、对展示层隐藏（同 A+ 私聊规则 9 的 strip 语义）。
- 目标回合的回复走目标聊天**现有的出站链路**（auto / tool_only、
  回复粒度、per-bot 间隔、失败恢复都不新开分支）；源对话不产生任何出站消息，
  只在工具结果里回报成功/失败。
- 交接触发的回合遵守目标聊天自己的工具档位：目标是群时群聊禁用名单照常生效；
  交接本身不豁免、不提升任何权限。

### R4 附件

- `attachments` 走 [bot-outbound-media.md](./bot-outbound-media.md) 同一条
  realpath 白名单 + 大小限额（图片 10MB / 文件 50MB），路径在目标回合注入前校验，
  非法路径整次交接拒绝（不部分投递）。
- 附件经现有 `deliverBotMedia` 链路发送到目标聊天；交接方聊天不发送附件。

### R5 幂等与顺序

- 工具调用本身由 Agent 回合串行保证一次一调；同一次调用产生的目标输入只入队一次
  （以工具调用 id 去重，复用入站去重的登记模式，TTL 内重复投递静默丢弃）。
- 目标对话忙时交接排队，不阻塞源对话回合结束；源对话回合可正常收尾。

### R6 UI 与配置

- 无新配置面、无新面板：工具始终存在于 IM 回合工具面，不做开关
  （要整体关闭用现有 allowedCommands / 工具档位体系之外的裁剪留到后续需求出现再做）。
- 对话列表 UI 不新增「交接」按钮——用户手动在目标聊天发消息即可，工具只服务 Agent。

## 状态所有者

```text
Agent IM 回合（源对话）
  │ handoff_message(target, message, context?, attachments?)
  ▼
handoff 服务（新，bots/ 下纯函数 + 服务方法）
  ├─ 目标校验：parseBotConversationKey + 本 bot 对话清单
  ├─ 附件校验：botMedia 白名单/限额（复用，不复制）
  └─ 入队：目标对话 messageQueue.enqueue（现有 owner）
              │
              ▼
     目标对话回合准入（enqueueInboundProcessing / botTurnGate，不变）
              │
              ▼
     目标出站链路（outboundDelivery / 回复粒度，不变）
```

- 不新增持久化文件；排队/去重状态沿用 bot-state 现有结构。
- handoff 校验逻辑为纯函数（目标解析、清单匹配、参数校验），便于单测。

## 验收场景

1. `packages/services/test/bot-handoff.test.ts`（新增）：
   - 目标存在：A 对话调用后，目标对话收到注入（含 context 与不可信标注），
     目标回合回复出现在目标聊天出站链路，源对话无出站；
   - 目标不存在 / 跨 bot：工具报错并回显可用目标，无入队；
   - 目标忙：入队排队，源回合正常结束；队满：报错含原因；
   - 非法附件路径：整次拒绝，目标无部分投递；
   - 同一工具调用 id 重复投递：第二次静默丢弃；
   - 目标为群：禁用工具档位按群规则生效。
2. 暴露面测试：桌面会话构造工具面不含 `handoff_message`；IM 回合包含。
3. 实机：企微私聊 A 中 Agent 交接到同 bot 的群 B，群里收到结合 B 历史的回复；
   目标不存在时报错文案可读。
4. `pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed`、
   `pnpm --filter @mode/services test` 通过。

## 非目标

- 不做跨 bot / 跨 Agent 交接；
- 不做桌面↔IM 交接（im-bot-rework.md N5 是独立决策）；
- 不做用户命令入口（用户决策：仅 Agent 工具）；
- 不做交接历史 UI / 审计面板；
- 不改 `enqueueInboundProcessing` 的串行与全局并发语义；
- 不做 MyAgents 0.5.4 的其他条目（发送模式统一、上下文收口——我们已对齐）。
