# bots 域接入形态裁决与迁移契约（W5 传输层收口）

> 状态：**已作废（2026-11-20）**。上位：`docs/plans/2026-10-29-full-rust-rewrite.md` §W5。
> 本裁决被 `docs/specs/im-sdk-runtime.md` 取代：IM 传输层改回集成官方 npm SDK，
> bots 域继续由 Node Host 持有；`crates/{bots-core,bots-adapters,dingtalk-stream,
> weixin-ilink,feishu-ws,wecom-ws}` 不进入产品依赖路径，仅作协议参考资料保留；
> 下表 B3–B5 取消。下文保留为决策历史，不得按本文实施。

## 1. 裁决：壳内接管 bots 域（拒绝 napi 桥）

| 方案 | 结论 | 理由 |
| --- | --- | --- |
| A. napi addon 供 TS `botsService` 调用 | **否决** | ① 终态要求发运集无 Node 运行时（W5/W6），napi 把 Node 保留为永久 owner，之后仍要再删一次；② napi 需按 Node ABI × 3 平台预编译分发，sidecar 发运复杂度上升；③ 连接生命周期（重连/心跳/串行 ack 队列）在 Rust，而事件消费与状态在 TS，形成第二套跨语言时序模型，违背「单一所有者」。 |
| B. 壳内接管 bots 域 | **采纳** | 与 W2（壳内频道默认集）和 W6（删 Node 宿主编排）同向；bot 长连/轮询与其它壳内频道一样归壳生命周期；配置/凭据已有 Rust 侧基础设施（`credential-store`、`data-root`）；频道级原子切换，可回滚。 |

**边界**：B 方案不改 IM 产品行为——`docs/specs/bot-*.md` / `im-bot-*.md` 等既有规范
仍是行为真源；Rust 实现逐条对齐，切换前 TS 实现保留为 oracle（与 W3 每域完成定义同口径）。

## 2. 阶段与交付

| 阶段 | 内容 | 状态 |
| --- | --- | --- |
| B1 | **seam 契约**：`crates/bots-core` —— provider 中立类型 + `ProviderAdapter` trait（镜像 TS `BotProviderAdapter` 的可移植子集），四通道 transport crate 与后续 service 层之间的唯一接缝 | ✅ 本波 |
| B2 | 四通道**入站解析移植**：`crates/bots-adapters`（transport crate 保持协议纯，适配层做「平台载荷 → provider 中立消息」投影 + 发送目标判定）。**B2a 钉钉**（`bot-dingtalk.test.ts` 6 条镜像）；**B2b 飞书/企微/微信已交付**——飞书 text/post/attachment/mention gating（Lark 域映射）、企微 text/mixed/voice/quote/媒体附件、微信紧凑投影 + 原始 item_list/直接附件（多别名、AES key、size）；**B2c 待做**：各通道发送路径（HTTP + token + 卡片/流式） | 🟡 B2a+B2b |
| B3 | bots service 核心（Rust 移植） | ⛔ 取消（2026-11-20 改向） |
| B4 | `bots` 频道接入与配置/凭据所有权移交 | ⛔ 取消（2026-11-20 改向） |
| B5 | 删除两个 IM SDK 依赖 | ⛔ 取消（反向：SDK 保留并支持在线升级） |

**切换纪律**：同一频道只有一个 owner（m4b §2/§5）。B4 之前 bots 归 Node Host；B4 打开
`bots` 频道后归壳；回滚 = 从开关移除并重启。配置/凭据不做双写：B4 的切换包含
`bot-config.v3.json` / `bot-state.v3.json` / `bot-bindings.v3.json` 的写所有权移交。

## 3. 契约（B1：`crates/bots-core`）

### 3.1 数据形状（对齐 `packages/shared/src/bots.ts` 与 `providers/types.ts`）

```text
ProviderKind = feishu | wecom | dingtalk | weixin
ChatType     = private | group
InboundAttachment { kind: image|audio|video|file, provider_id?, download_url?, aes_key?,
                    filename?, mime_type?, metadata: JSON }
InboundMessage { provider, bot_id, message_id, text, chat_type,
                 chat_id?, provider_user_id, display_name?, context_token?,
                 attachments[], raw: JSON }
OutboundMessage { provider_user_id, context_token?, text }
OutboundMedia   { kind: image|file, filename, mime_type, data, caption? }
TypingTarget    { provider_user_id, provider_message_id?, context_token? }
StreamingCardState { provider_user_id, locale?, blocks[]（message|tools）, status }
ProviderError   { Unsupported(method) | Platform(message) }
```

### 3.2 `ProviderAdapter` trait（TS `BotProviderAdapter` 的可移植子集）

必选：`provider` / `parse_callback` / `send`。
可选（默认 `Unsupported` 或安全缺省）：`send_media`、`start_typing`/`stop_typing`、
`refresh_context_token`、`can_write_streaming_reply`（缺省 true，对齐 TS
「未实现视为总是可写」）、`supports_streaming_reply`、`create_streaming_reply_card` /
`update_streaming_reply_card` / `split_streaming_reply_card_states`、
`acknowledge_callback`、`download_attachment`。

不迁入 trait（B3 服务层职责或平台专有卡片语义）：`test`/`resolve_name`/`sync_commands`/
`resolve_actor_display_name`/`prepare_callback_payload`/`notifyTaskLifecycle`/transient
interaction card——卡片/命令同步属服务层与后续波次。

## 4. 真机验收门禁（B4→B5）

- 每个通道一个真实应用：消息往返、群聊 @、媒体收发、断网重连、服务端重投去重、
  context/token 过期恢复各一轮；
- 与 TS 通道**不同 bot 并行**跑同一脚本，行为对拍无差异；
- 20 项任务清单（`host-ownership-switch.md` §7 口径）全绿；
- 关闭 `bots` 开关回 Node 无回归——之后才允许 B5 删除 SDK 依赖。

## 5. 风险

1. **9.3k 行服务层**是主工程量：先按 bot-*.md 分域移植（命令/授权/群聊/绑定/回复），
   每域配 TS 对拍，不一次性迁移；
2. 四通道入站解析的平台字段形状漂移：B2 以自家 TS + 真机夹具为准，不猜字段；
3. 流式回复的 req_id 生命周期（企微）与会话归属绑定：trait 的
   `can_write_streaming_reply` 保持 TS 语义（拿不到入站帧 req_id 时走离散气泡）。
