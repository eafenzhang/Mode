# M3 生态域：IM bot 验收现状与决策（2026-10-18）

> 本文件是 M3「IM bot 逐通道」的**盘点 + 决策记录**，与
> `2026-10-18-m3-lan-remote-status.md`（LAN/远程域）同一口径：先跑现有实现与测试，
> 再判断 M3 是否需要新代码，而不是按路线图的事务性动词直接开写 Rust。
>
> 结论先行：**IM bot 由 Host sidecar 复用现有 `packages/services` 实现，M3 不在
> Rust 侧新增 `crates/bots`；逐通道的剩余工作是真账号验收，属必须由人完成。**
> 路线图 M3 节「飞书/企业微信/钉钉走 HTTP+WS 直连（reqwest/tungstenite）」的
> Rust 重写口径由本文件改为延后（见 DR-IM-1）。

## 归属与架构证据

- `packages/desktop/src/host/index.ts:973` 在 **Host 进程**里取 `IBotsService` 并实例化
  bot 运行时；Tauri 侧 Host 是同一个 Node sidecar（M3 生态计划的 DR-M3-1），因此
  Tauri 壳启动 sidecar 后 bot 能力**原样可用**，不需要另起一套运行时。
- 机器人服务与桌面 renderer 之间只有既有 RPC/服务面，没有 Electron 专有依赖：
  `packages/services/src/bots/**` 只依赖 `ws`、`fetch` 与两个官方 SDK。
- 既有数据格式（`installed` 配置、绑定表、群历史）在 `packages/services` 内维护，
  迁移到新栈时按 AGENTS.md「每个窗口一个 Host、远程由窗口连接注册表管理」复用。

## 通道清单与传输方式

| 通道 | 传输 | 实现位置 | 深依赖 | M3 处理 |
| --- | --- | --- | --- | --- |
| 飞书 / Lark | WS 长连接 + 开放平台 HTTP | `feishuChannelRuntime.ts`、`providers/feishuProvider.ts` | `@larksuiteoapi/node-sdk` | sidecar 复用（SDK 深依赖，按路线图原规则「先走 sidecar」） |
| 企业微信 | WSS（`openws.work.weixin.qq.com`） | `wecomChannelRuntime.ts`、`providers/wecomProvider.ts`、`wecomConnection.ts` | `@wecom/aibot-node-sdk` | sidecar 复用 |
| 钉钉 | Stream 长连接，自建 WS 客户端 | `dingtalkChannelRuntime.ts`、`providers/dingtalkProvider.ts` | 无（`ws`） | sidecar 复用；若 M4 要把运行时移出 sidecar，可优先 Rust 化 |
| 微信（个人/客服） | HTTP 轮询/回调 + 自建运行时 | `weixinChannelRuntime.ts`、`providers/weixinProvider.ts` | 无 | sidecar 复用 |
| Telegram | HTTP 长轮询（存量通道，不再新建） | `telegramChannelRuntime.ts`、`providers/telegramProvider.ts` | 无 | 保留存量可编辑，不做迁移 |
| Webhook | 入站 HTTP | `providers/webhookProvider.ts` | 无 | 保留存量，不做迁移 |
| AstrBot / Discord | 已下线 | 适配器为 `null` | — | 仅保留历史配置解析字面量 |

`lark` 与 `feishu` 共用同一 provider（`botsService.ts` 的 `providers` 表），
`weixin` 的自有身份去重见 `weixinSelfId.ts` 与 spec `weixin-provider-self-id.md`。

## 行为规格（沿用，不重写）

既有 spec 即本域的验收基线，Rust 化或迁移时不得改变语义：

- `bot-state-ownership.md`（状态所有者与唯一写入路径）
- `bot-workspace-turn-admission.md` / `workspaceTurnGate.ts`（busy 准入与串行）
- `bot-workspace-switch-unbind.md`、`bot-binding-indicators.md`、`bot-binding-menu-display.md`
- `bot-reply-granularity.md`、`bot-reply-text-format.md`、`bot-queued-message-attachments.md`、
  `bot-attachment-cache-retention.md`、`bot-outbound-delivery.md`、`bot-outbound-target.md`
- `im-bot-group-chat-alignment.md`、`im-bot-onboarding.md`、`im-bot-remote-workspace-binding.md`
- `im-typing-feedback.md`、`wecom-reply-feedback.md`、`weixin-provider-self-id.md`

## 测试证据（2026-10-18 实跑）

命令（在 `packages/services` 下）：

```bash
pnpm exec tsx --test test/bot-*.test.ts test/wecom-registration.test.ts
```

结果：**149 tests / 149 pass / 0 fail**，覆盖 25 个 `bot-*` 测试文件 +
`wecom-registration.test.ts`，包括：

- 会话绑定与工作区绑定（`bot-workspace-binding`、`bot-conversation-session`、
  `bot-remote-workspace-identity`、`bot-workspace-turn-gate`）；
- 回复链路（`bot-reply-granularity`、`bot-reply-segments`、`bot-reply-text-format`、
  `bot-streaming-edit`、`bot-outbound-delivery`、`bot-text-media`）；
- 通道特性（`bot-dingtalk`、`bot-wecom-typing`、`bot-weixin-typing`、
  `wecom-registration`）；
- 群聊与历史（`bot-group-chat`、`bot-group-history-store`、`bot-group-prompt`）；
- 队列与心跳（`bot-message-queue`、`bot-heartbeat`）；
- 绑定隔离与并发（绑定表与设置文件解耦、多机器人、cap=2 的 FIFO 准入）。

## DR-IM-1：M3 不在 Rust 侧重写 IM bot（2026-10-18）

| 选项 | 含义 | 代价 |
| --- | --- | --- |
| **(a) Host sidecar 复用（采纳）** | Tauri 壳 spawn 的 Node sidecar 继续承载 `packages/services` 的 bot 运行时；M3 只验证既有行为 | 与 LAN/远程域一致，无重复状态；两个官方 SDK 依赖留在 sidecar |
| (b) `crates/bots` 用 reqwest/tungstenite 重写 | 飞书/企微/钉钉/微信各写一套 Rust 客户端 | 与 sidecar 内 8.9k 行既有实现重复；SDK 深依赖通道仍需 sidecar；clean-room 下不能照抄实现 |

采纳 (a) 的依据：M3 生态计划已把 `packages/services` 整体放进 sidecar（DR-M3-1），
IM bot 没有独立于 Host 的生命周期；重写会违反 AGENTS.md「避免重复状态和多条写入路径」。
(b) 只在 M4/M5 把 bot 运行时移出 sidecar 时重新评估；届时优先钉钉（无 SDK 依赖）。

## 仍未完成项

**必须由人完成（真账号/真设备）**：

1. 飞书：创建企业自建应用、配置长连接与权限、扫码授权、群/私聊各发一轮含附件的消息；
2. 企业微信：智能机器人回调校验、WSS 连接、流式编辑的最终形态；
3. 钉钉：Stream 长连接 ticket、单 AppKey 单机器人独占校验；
4. 微信：扫码登录/回调、自有身份去重（`weixin-provider-self-id`）；
5. 两台机器上的「手机发消息 → 桌面 Host 执行 → 回复到手机」端到端链路。

这些都需要真实凭据与外部平台后台，自动化无法替代，也不应伪造。

**可自动执行的后续（下一轮候选）**：

- 插件市场迁移收口（`plugin-store-availability` spec 的场景盘点与现有测试跑通）；
- Web / 手机远控域验收（与 LAN 域同口径：先跑现有测试再决定是否立项）；
- TUI（`ratatui`）——需要先确认旧 TUI 的场景清单，再判断是否属 M3。
