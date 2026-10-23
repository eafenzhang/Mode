# 微信私聊默认开放（扫码即可用）

状态：已批准（2026-10-21，用户指令「执行优化」）。
关联：[im-bot-onboarding.md](./im-bot-onboarding.md)、[weixin-provider-self-id.md](./weixin-provider-self-id.md)、
[im-bot-agent-channel-aplus.md](./im-bot-agent-channel-aplus.md)、[bot-outbound-delivery.md](./bot-outbound-delivery.md)、
[bot-workspace-turn-admission.md](./bot-workspace-turn-admission.md)。

## 背景

用户问题（2026-10-21）：「微信为何还需绑定用户」。排查结论：微信扫码注册只返回
`ilink_bot_id`（机器人自身 id）和凭据，**不返回扫码人的微信用户 id**；服务层无法把
「扫码的人」自动认定为绑定用户。现状缺省 `privateChatMode = bound_users`，于是新建的
微信 Bot 一打开面板就要求 `/bind`，而 `im-bot-onboarding.md` 的 R2 / R3-2 还写着
「微信不设绑定段」，文档与实现不一致。

参照产品 MyAgents 的行为（只对行为，未读实现）：

- 微信走 OpenClaw 插件扫码登录（`@tencent-weixin/openclaw-weixin`），不是 bot 注册；
- `CHANGELOG.md:1887-1888`：「微信向导多余 BIND 口令：QR 登录插件不再显示 BindCodePanel，
  替换为『扫码即可使用』提示」「Channel 详情页已绑定用户为空：QR 登录 dmPolicy=open
  不需要白名单，UI 对齐」；
- 内置 Telegram 等平台才是「空白名单拒绝所有消息 + QR 绑定加入白名单」
  （`specs/tech_docs/im_integration_architecture.md:271-274`、`:874-900`）。

我们的微信渠道与 MyAgents 的身份语义不同（iLink bot 是独立 bot 身份），但产品目标一致：
扫码接入后不应再被一个默认的绑定步骤挡住。显式收紧到 `bound_users` 的能力保留。

## 产品规则

### R1 缺省值按 provider 解析

私聊方式的缺省值唯一由 `resolveBotPrivateChatMode`（`packages/shared/src/bots.ts`）解析。

2026-10-23 起降级为**存量推导规则**：新建微信 Bot 缺省改为 `whitelist`
（[bot-access-mode.md](./bot-access-mode.md) R5）；本表只用于无 `accessMode` 字段的存量配置。

| provider | 缺省 `privateChatMode` | 含义                         |
| -------- | ---------------------- | ---------------------------- |
| weixin   | `all_users`            | 任何私聊用户都可驱动，无需绑定 |
| 其它     | `bound_users`          | 仅绑定用户 / `allowedUsers` 可驱动 |

调用方（服务端准入、主动投递目标、UI 展示）必须使用该函数，**不得**在各处重写
`privateChatMode ?? "bound_users"`。显式配置永远优先。

### R2 显式 `bound_users` 保留

用户在面板里把「私聊方式」切回「绑定用户」后：

- 入站仍按 `/bind` + `providerUserId` / `allowedUsers` 收紧；
- 面板恢复「已绑定用户」列表与绑定码入口；
- 不迁移、不清空既有 `providerUserId` / `allowedUsers`。

`/bind` 命令、绑定码和 `handleBind` 都不删除，只是微信的默认路径不再经过它。

### R3 all_users 只影响私聊

`all_users` 的放行范围仍是「私聊」；群聊继续走群授权闸门
（`im-bot-agent-channel-aplus.md` 私聊规则 2、群聊规则 6）。微信不支持群聊，
群卡片不渲染。

### R4 主动投递目标不变

`resolveBotProactiveUserId` 的优先级保持：

```
绑定用户 providerUserId
  → 当前私聊对话 conversationId
  → all_users 时最近一条私聊对话
  → 跳过本轮
```

微信在 `all_users` 且 `providerUserId` 为空时，首条入站消息会建立私聊对话，
此后心跳 / 任务流镜像 / 媒体投递沿用该对话（`weixin-provider-self-id.md` 规则 5/6）。
任务流镜像订阅 `ensureContextStreamWatch` 也复用 `findBoundUser`：`all_users` 下
微信任务会正常建立订阅，不再因缺少 `providerUserId` 而跳过。

### R5 UI 不新增微信专属控件

- 保留「私聊方式」下拉，默认显示「全部用户」；
  （2026-10-23 修订：下拉移入访问模式卡（bot-access-mode.md R6））
- 按现有 `isBoundUsersMode` 分支，「全部用户」下自然隐藏「已绑定用户」和绑定码区域，
  不新增 `if provider === "weixin"` 的展示分支；
- 新建 Bot 时由 `createDraftBot` 显式落盘 `accessMode: "whitelist"`（全渠道），
  存量配置未写该字段时由 R1 的解析函数兜底；
- 微信扫码成功提示保持「发送任意消息激活」，不承诺自动绑定。

### R6 同步修正过期 spec

> 2026-10-23 起新建渠道缺省 whitelist，本文件旧断言降级为存量描述。

`im-bot-onboarding.md` 的 R2 / R3-2 与验收项 3 曾写「微信不设 `/bind` 阶段 / 不出现
`/bind` 入口」。该表述描述的是当时的 UI 行为，与 `weixin-provider-self-id.md` 规则 8
（微信可手动 `/bind` 真实用户）和当前实现不一致。本次统一改为：

- 微信默认不需要绑定（`all_users`）；
- 显式 `bound_users` 时保留 `/bind` 与绑定 UI；
- 状态行的「凭据 / 激活」两段呈现不变。

## 状态所有者

```text
createDraftBot（UI）──► bot-config.v3.json: privateChatMode（显式缺省值）
存量配置未写 ─────────► resolveBotPrivateChatMode（纯函数）
                              │
        ┌─────────────────────┼──────────────────────┐
        ▼                     ▼                      ▼
findBoundUser         resolveBotProactiveUserId   BotPrivateChatCard / BotGroupChatCard
（入站准入）           （主动投递目标）             （展示与绑定入口）
```

- 配置唯一写入方仍是 `repo.writeConfig` / `saveBot`；本次不新增字段、不做状态迁移。
- `resolveBotPrivateChatMode` 是无状态纯函数，反复调用结果一致。
- 心跳、流镜像、媒体投递不新增目标来源，只沿用既有回落链。

## 验收场景

1. `packages/services/test/bot-private-admission.test.ts`：
   - 微信未配置 `privateChatMode` → 陌生私聊放行、群聊拒绝；
   - 微信显式 `bound_users` → 陌生私聊拒绝；
   - 企微未配置 → 陌生私聊拒绝（非微信默认不变）。
2. `packages/ui/test/botWeixinOpenDm.test.ts`（源码契约）：
   - `BotPrivateChatCard` / `BotGroupChatCard` 使用 `resolveBotPrivateChatMode`；
   - 新建 Bot 显式落盘解析后的缺省值。
3. 实机：新建微信 Bot 详情页「私聊方式」显示「全部用户」，不再出现「绑定」按钮；
   微信里用任意账号发消息可驱动；切回「绑定用户」后绑定码与已绑定用户列表恢复。
4. `pnpm typecheck`、`pnpm lint`、`pnpm --filter @mode/services test` 通过。

## 非目标

- 不删除 `/bind`、绑定码、`bound_users` 模式；
- 不改变群聊授权闸门，不给微信增加群聊能力；
- 不迁移存量微信 Bot 的 `providerUserId` / `allowedUsers`；
- 不改变 `weixinActivatedAt` 的激活语义；
- 不引入「自动绑定首个发送者」—那会把任意陌生人的第一条消息变成主人授权。
