# IM 对话等待期反馈（思考期必有可见提示）

状态：已批准（2026-10-11）。关联：[wecom-reply-feedback.md](./wecom-reply-feedback.md)（企微占位流细节）、[bot-workspace-turn-admission.md](./bot-workspace-turn-admission.md)（准入排队）。

## 背景

用户在 IM 里连续提问时看不到任何等待提示，会误认为对话已经结束（实机截图：微信侧提问后长时间无反馈）。排查确认三类缺口：

1. `watchTaskStream` 在订阅残留的 early-return 分支**跳过 `startTyping`**——上一轮终态事件丢失等场景下，续问全程没有任何反馈；
2. 微信 `/sendtyping` 链路**全静默**：无 `typing_ticket` 直接 return、请求失败被 `.catch(() => undefined)` 吞掉，且没有取消路径（回复后原生「对方正在输入…」可能滞留）；
3. 企微 `stopTyping` 冻结后 `stopAnimation` 引用从未清理，续动画分支变成死代码（与 wecom-reply-feedback.md 承诺矛盾）。

## 产品规则

- **必有反馈**：每个 IM 对话，回合起跑（`watchTaskStream` 建立**或复用**订阅）到回复落地/任务终态之间，必须有可见的等待反馈。
- **失败可观测**：反馈链路的失败不允许静默——拿不到票、请求失败必须以 `warn` 落日志（可恢复异常），并保留一次自动重试。
- **按时让位**：任务终态、短命令回复完成时反馈必须立即取消；不得残留（微信用 `/sendtyping status:2`）。
- **不被后续消息打断**：长回合进行中，同一聊天的后续入站完成（排队提示、短命令回执）不得取消进行中的 typing；guard 口径为「同 bot + 同聊天」，不再是「同一条入站消息」。

### 各 provider 机制

| provider       | 等待反馈                                                     | 本轮动作                                               |
| -------------- | ------------------------------------------------------------ | ------------------------------------------------------ |
| 企微 wecom     | 占位流（内容 MyAgents 同款；动态感来自企微 PC 原生 loading） | 改为开流一次的占位，移除 500ms 打点动画                |
| 微信 weixin    | `/sendtyping` 原生「正在输入…」                              | 票缓存 + 失败强刷重试 + 失败 warn + 补 `status:2` 取消 |
| 飞书 feishu    | Typing reaction                                              | 仅受益于 guard 口径修正                                |
| Telegram       | chatAction                                                   | 仅受益于 guard 口径修正                                |
| 钉钉 / webhook | 无                                                           | 非目标（另立任务）                                     |

- 微信不发占位消息：iLink 无公开的编辑/更新消息接口（官方包与社区 SDK 一致，`message_state` 只有 FINISH=2 被实际发送），发 ··· 占位会在历史里每轮留一条残留。
- 回合内既有反馈保持不变：本对话任务运行中的排队已有「已排队第 N 位」文案。

## 验收场景

1. 单测（`bot-weixin-typing.test.ts`）：ticket 缓存复用（多次 sendTyping 只打一次 getconfig）、发送失败强刷新票重试一次、无票抛错（由上层 warn，不静默）、`stopTyping` 发 `status:2`。
2. 源码断言：`watchTaskStream` 订阅残留分支调用 `startTyping`；`startTyping` 的 sendTyping 分支同时登记 `typingTargets`（terminal 取消与 stopInboundTyping guard 依赖它）；`stopInboundTyping` guard 按 bot+聊天匹配。
3. 企微（`bot-wecom-typing.test.ts`）：占位为 MyAgents 同款静态文案（`WECOM_TYPING_PLACEHOLDER`，开流一次、无循环刷新帧）；`stopTyping` 不收口占位流；`startTyping` 重复/恢复触发保持幂等单流。
4. `packages/services` 全量测试、`pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed` 全绿。

## 非目标

- 钉钉 / webhook 的等待反馈（当前无任何 typing 能力，需单独设计）。
- 微信 `message_state=1` 流式实验（公开世界无人验证服务端是否接受，留作后续探针）。
- 桌面会话时间线的打点气泡（`conversation-typing-bubble.md` 单独维护）。
- 跨对话 turnGate 静默等待提示（bot-workspace-turn-admission.md 的 v1 已知限制，文案留待产品决策）。

## 本轮验证状态

- 已验证：`packages/services` 全量 268 通过 / 0 失败（8 个存量跳过）；其中新增
  `bot-weixin-typing.test.ts` 4 项、`bot-typing-feedback.test.ts` 3 项、
  `bot-wecom-typing.test.ts` 新增续动画 1 项；`pnpm typecheck`、`pnpm lint`（0 errors）、
  `pnpm architecture:check --changed` 全绿。
- **待实机**：微信原生「正在输入…」在真实聊天里的观感（开/关时序、取消是否及时）需要
  配置好的微信 bot 走一轮真实对话确认；日志侧以 `bot typing … failed` 的 warn 是否出现为准。
