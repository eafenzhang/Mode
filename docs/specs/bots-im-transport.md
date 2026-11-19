# IM 四通道传输层 Rust 自实现（W5 第一阶段）

> 状态：v1（2026-11-20）。上位计划：`docs/plans/2026-10-29-full-rust-rewrite.md` §W5
> 「IM 四通道 Rust 自实现：不再复用 SDK」。产品行为真源不在这份 spec——
> `docs/specs/bot-*.md` / `im-bot-*.md` / `wecom-reply-feedback.md` 等既有规范继续有效。

## 1. 范围与切分线

**只替换传输/协议层，不动产品逻辑**。勘察结论（2026-11-20，file:line 为证）：

| 通道 | 接入方式 | 协议面归属 | 行数量级 |
| --- | --- | --- | --- |
| 飞书 | WS 长连 = SDK `WSClient`+`EventDispatcher`（`feishuProvider.ts:1675-1778`）；**发送全走 OpenAPI HTTP，不经 SDK** | SDK 封装 → 待重写 | ~2650 |
| 企微 | WS 长连 = SDK `WSClient`，且**发送/流式/媒体也走 WS**（`sendMessage/replyStream/replyMedia/uploadMedia/downloadFile`，`wecomProvider.ts:314-607`） | SDK 封装 → 待重写（协议面最大） | ~1320 |
| 钉钉 | **已是手写协议**：`POST /v1.0/gateway/connections/open` → 裸 WS JSON 帧（`dingtalkChannelRuntime.ts`） | 自有 TS → 平移 | ~840 |
| 微信 | **已是手写协议**：HTTP 长轮询 `/ilink/bot/getupdates` + AES-128-ECB 附件（`weixinProvider.ts`） | 自有 TS → 平移 | ~1490 |

- `botsService.ts`（9334 行）的全部产品逻辑（命令/授权/群历史/回合准入/流式卡片/恢复链）
  保持 **TS 为行为真源**，Rust 不「顺便重构」；接缝是 `BotProviderAdapter`
  （`providers/types.ts:66-145`）。
- 配置/凭据/状态三份 JSON（`bot-config.v3.json` 等）与 `credentials.json` 加密存储
  继续由 TS 侧持有，避免双写。
- SDK 懒加载守卫（`bot-sdk-lazy-import.test.ts`）在通道替换完成后逐一改为「SDK 依赖删除」门禁。

## 2. wire 事实源矩阵（关键裁决）

| 通道 | wire 事实源 | 状态 |
| --- | --- | --- |
| 钉钉 Stream | **官方协议文档**（open-dingtalk.github.io `docs/learn/stream/protocol`）+ 自家 TS 实现 | ✅ 齐，可离线对拍 |
| 微信 iLink | 自家 TS 实现（手写 HTTP）+ 官方接口文档 | ✅ 齐，可离线对拍 |
| 飞书 WS | SDK 内部协议，**官方文档不公开 wire** | ⬜ 待真机抓包（需真实应用凭据）或用户对「参考第三方 SDK 源码是否违反 clean-room 精神」另行裁决 |
| 企微 WS | 同上（`@wecom/aibot-node-sdk` 的 WS 协议未公开） | ⬜ 同上 |

因此第一波 = **钉钉 Stream**（事实源最完整），第二波 = 微信 iLink 平移，
飞书/企微排在 wire 事实源落定之后。**不读第三方 SDK 源码**（与 clean-room 精神一致），
飞书/企微的 wire 以真机抓包为准。

## 3. 钉钉 Stream 协议事实（官方文档 + 自家 TS，`dingtalkChannelRuntime.ts`）

1. `POST {api_base}/v1.0/gateway/connections/open`：请求 `{clientId, clientSecret,
   subscriptions:[{type:"CALLBACK",topic:"/v1.0/im/bot/messages/get"},{type:"EVENT",topic:"*"}]}`
   （TS :85-99）；响应 `{endpoint, ticket}`；**ticket 90s 有效且只能建一条连接**（官方）。
2. WS 连接 `{endpoint}?ticket={ticket}`（TS :248）。
3. 下行帧 JSON：`{specVersion:"1.0", type, headers, data}`；`headers` 含
   `contentType/messageId/topic`（EVENT 另有 eventType/eventId 等，原样透传不做建模）。
4. `SYSTEM`/`topic=ping` → 原样回 `{code:200, headers, message:"OK", data}`（headers/data
   透传，TS :166-172）；官方：收到 ping 直接响应，data 回带 opaque。
5. `SYSTEM`/`topic=disconnect` → 关闭连接交给重连循环（TS :174-183；官方：服务端定时优雅
   断开，静默 10s 后断 TCP，客户端无需响应）。
6. `CALLBACK`/`EVENT` 且有 `messageId` → 回 ACK `{code:200,
   headers:{contentType:"application/json",messageId}, message:"OK",
   data: EVENT?"{\"status\":\"SUCCESS\",\"message\":\"success\"}":"{}"}`（TS :187-199；
   官方：EVENT 的 data 必须含 status SUCCESS/LATER，不 ACK 会超时重推）。
7. 仅 `CALLBACK` 且 topic 为 `/v1.0/im/bot/messages/get`（含尾斜杠变体）→ 解析 `data`
   JSON 里的 `msgId`，非重复才派发 `{botId, frame}`（TS :201-229）；data 解析失败直接
   忽略（ACK 已发）。服务端可能重复推送 → 去重是**必须**（官方 + TS）。
8. 读超时：**120s 无任何帧判死重连**（TS :29,264-273；官方未给推荐值，120s 是自家决策）。
9. 重连：注册失败 → 状态 error + 5s 重试（TS :234-244）；连接结束后**健康连接（活过
   30s）重置退避，否则指数退避 1s→60s 上限**（TS :300-312）。
10. 去重：内存 Map + 落盘（200 条 / 24h TTL / 500ms 防抖持久化，跨重启覆盖服务端重投窗口，
    TS :34-36,107-146）。

## 4. 第一波交付：`crates/dingtalk-stream`

纯 Rust 库（不接产品进程），逐条对齐 §3：

- `frame.rs`：下行帧解析 + **单一纯函数** `handle_frame`（返回 回帧/派发/关闭 三元结果），
  行为与 TS `handleTextFrame` 逐条对齐；上/下行帧序列化保持 TS 的字段形状与顺序。
- `dedupe.rs`：`DedupeSet`（插入序淘汰最老 + TTL + 200 上限），持久化钩子由运行时层
  防抖触发。
- `open.rs`：`build_open_request`（纯函数）+ `StreamOpen` trait（可注入假注册器）+
  `HttpStreamOpen`（reqwest，ring provider 自装，与 workspace 惯例一致）。
- `runtime.rs`：单 bot 连接生命周期循环（open→WS→帧循环→重连），读超时/退避/健康重置
  可注入（测试用短时长）；status 上报枚举对齐 TS 状态；派发与去重持久化为注入回调。
- 测试：帧处理纯函数、去重、退避单测 + 本地假网关注册器 + 本地假 WS 服务端的离线
  集成测试（ping/EVENT/CALLBACK/disconnect/读超时/重连）。

**接入方式未决**（后续波次裁决）：Rust 传输层以何种形态服务 `botsService`——
napi 供 TS 调用 vs 壳内接管 bots 域；两者都不影响本 crate 的协议面与对拍。

## 5. 验收

1. 离线：`cargo test -p dingtalk-stream` 全绿（纯函数 + 假 WS 集成）。
2. 行为对齐：spec §3 每条在代码内有对齐注释（TS file:line）。
3. 真机（后续）：真实钉钉应用凭据下与 TS 通道并行跑（不同 bot），消息往返、断网重连、
   服务端重投去重各验一轮；通过后才谈 TS 通道退役。
