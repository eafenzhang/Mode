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
| 钉钉 Stream | **官方协议文档**（open-dingtalk.github.io `docs/learn/stream/protocol`）+ 自家 TS 实现 | ✅ 齐，可离线对拍（已交付 `crates/dingtalk-stream`） |
| 微信 iLink | 自家 TS 实现（手写 HTTP）+ 官方接口文档 | ✅ 齐，可离线对拍（已交付 `crates/weixin-ilink`） |
| 飞书 WS | **SDK 接口面提取**（`@larksuiteoapi/node-sdk@1.61.1`，MIT）+ 真机抓包验收 | ✅ 2026-11-20 裁决落地（见下），可开工 |
| 企微 WS | **SDK 接口面提取**（`@wecom/aibot-node-sdk@1.0.7`，MIT）+ 真机抓包验收 | ✅ 同上，可开工 |

**wire 事实源裁决（2026-11-20，用户确认）**：飞书/企微的 WS wire 由厂商 SDK 封装且官方
文档不公开；经定性（第三方厂商 SDK 不属 clean-room 政策的上游衍生实现、均 MIT、不在
0% 重合度基线内、wire 帧结构属政策自身的「接口面」定义），裁决为**允许按接口面口径
提取**——读 SDK 源码只限提取 wire 协议事实（帧字段/形状/顺序/常量/错误语义），必须
带 SDK file:line 引用写入对应 spec；实现只依据 spec 编写，禁止粘贴 SDK 代码结构；
真机抓包保留为验收层。纪律条款已写入 `docs/specs/clean-room-policy.md`「接口面优先级」
与 PR checklist。

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

## 4b. 第二波交付：`crates/weixin-ilink`（微信 iLink 请求面）

**边界（重要）**：第二波只平移「请求面」——getupdates 长轮询与游标、sendmessage 构造、
typing 票、getconfig 探测、CDN 附件 AES 解密。**入站消息的语义解析**（附件 kind 推断、
用户/群字段投影）留在 TS callback 管线——Rust 版 `getupdates` 返回**原始消息数组**与
`next_buf`，不做 `buildInboundMessage` 投影。iLink 无官方公开文档，**事实源 = 自家 TS**
（`weixinProvider.ts` / `weixinChannelRuntime.ts`，file:line 见下）+ 真机验收。

协议事实（TS file:line 为证）：

1. 基址 `https://ilinkai.weixin.qq.com` + 前缀 `/ilink/bot`，全部 POST（`weixinProvider.ts:16-17,176-181`）。
2. 请求头：`content-type: application/json`、`AuthorizationType: ilink_bot_token`、
   `Authorization: Bearer <token>`、`X-WECHAT-UIN`（随机 u32 的十进制字符串 base64，
   TS :110-121）。
3. 所有请求体首键注入 `base_info: { channel_version: "2.0.0" }`（TS :123-131）。
4. 业务码判定：`ret !== 0 || errcode !== 0` 即失败，errmsg/message 优先取（TS :200-219）。
5. 失败分类（TS :138-159）：HTTP 401/403 → `session_expired`；HTTP 429/≥500 →
   `retryable`；errmsg `"prepare failed"` → `context_expired`；token 缺失 →
   `session_expired`；网络/超时 → `retryable`（TS :186-194）。
6. `/getupdates`：请求 `{get_updates_buf: buf ?? ""}`，**超时 90s**（长轮询挂起，
   TS :22,709-718）；响应容器取 `data`，消息数组键 `msgs/messages/updates/items/list`
   （TS :548-558），游标键 `get_updates_buf/buf/next_buf/nextBuf/getUpdatesBuf/syncKey`
   （TS :565-575）。**游标必须等本批消息全部进入业务处理后持久化**（TS :161-165 Bugfix，
   顺序约束：先处理 → 再写 buf）。跨进程单消费者：轮询锁（TS :72-99，多窗口互斥）。
7. `/sendmessage`：`msg{from_user_id(=providerSelfId ?? providerUserId，spec
   weixin-provider-self-id), to_user_id, client_id("mode-weixin-<uuid>"),
   message_type:2, message_state:2, context_token?, item_list:[{type:1,
   text_item:{text}}]}`（TS :815-836）。
8. 文本降级：iLink 只发纯文本——markdown 降级（剥代码围栏行/行内代码/粗体/标题标记，
   链接转 `text (url)`；单个星号/下划线不碰；幂等，`botText.ts:154-172`）+
   LF→CRLF 归一（TS :597-605）。
9. typing：`/getconfig {ilink_user_id, context_token?}` 拿 `typing_ticket`（按
   `bot:用户` 缓存）→ `/sendtyping {ilink_user_id, typing_ticket, status:1|2}`；
   sendtyping 失败按票过期处理——强刷一次再试，仍失败上抛（TS :731-795）。
10. `/getconfig`（无参）是连通性探测与会话续期入口（TS :808,845-854）。
11. 附件下载：CDN `downloadUrl` 原始字节 → **AES-128-ECB + PKCS7** 解密（key = base64
    16 字节，TS :21,85-99）。

crate 结构：`headers.rs`（头/UIN/构造）、`classify.rs`（失败分类纯函数）、
`updates.rs`（容器/游标解析纯函数）、`send.rs`（sendmessage 构造 + markdown 降级 +
CRLF）、`typing.rs`（票缓存与重试流程）、`aes.rs`（ECB+PKCS7 解密，仅 `aes` crate
手写 ECB/PKCS7，不引 ecb 依赖）、`http.rs`（`WeixinHttp` trait 注入 + reqwest 生产实现）。
测试：纯函数单测 + 本地最小 HTTP 假服务端集成（头/体/业务码/401 分类/typing 重试）。

## 4c. 第三波交付：`crates/feishu-ws`（飞书 WS 长连客户端）

**wire 事实源**：`@larksuiteoapi/node-sdk`（MIT，Lark Technologies Pte. Ltd.）——按
§2 裁决以接口面口径提取。引用均为 `lib/index.js`（本地安装版 **1.61.1**；仓库 pin
1.64.0，wire 面如有变更需重提）+ 行号。**实现只依据本节编写**。

### 5.1 连接注册（HTTP）

1. `POST {domain}/callback/ws/endpoint`；domain 映射：Feishu → `https://open.feishu.cn`、
   Lark → `https://open.larksuite.com`（:216-222, :140-142, :85191-85193）。
2. 请求体 `{AppID, AppSecret}`，请求头 `locale: zh`（网关消费），单请求超时 15s
   （:85329-85341）。
3. 响应 `{code, data: {URL, ClientConfig}, msg}`；ErrorCode：ok=0、system_busy=1、
   forbidden=403、auth_failed=514、internal_error=1000040343、exceed_conn_limit=
   1000040350（:85260-85267）。`code !== 0` → 注册失败进重试。
4. `URL` 为 WS 连接串（含 ticket 等），其 query 带 `device_id` 与 `service_id`
   （qs.parse，:85348）；`service_id` 用于 ping 帧的 `service` 字段。
5. `ClientConfig`（秒）：`PingInterval / ReconnectCount / ReconnectInterval /
   ReconnectNonce`（:85353-85356）；无配置时缺省 ping 120s、reconnectCount **-1（无限）**、
   reconnectInterval 120s、reconnectNonce 30s（WSConfig 构造，:85165-85175）。

### 5.2 WS 帧：protobuf `pbbp2.Frame`（二进制）

1. 字段（encode :84826-84844）：1 `SeqID` uint64、2 `LogID` uint64、3 `service` int32、
   4 `method` int32、5 repeated `Header{1 key string, 2 value string}`、6
   `payloadEncoding` string、7 `payloadType` string、8 `payload` bytes、9 `LogIDNew`
   string（均可选，编解码按 hasOwnProperty 宽容处理；decode required = SeqID/LogID/
   service/method，:84921-84928；未知字段跳过）。
2. `method`：control=0、data=1（:85268-85272）。
3. `HeaderKey`：type / message_id / sum / seq / trace_id / biz_rt / handshake-status /
   handshake-msg / handshake-autherrcode（:85273-85284）。
4. `MessageType`（HeaderKey.type 的值）：event / card / ping / pong（:85285-85291）。
5. 发送：整帧编码为二进制经 WS 发出（sendMessage，:85648-85659）。

### 5.3 会话行为

1. WS open 后立即起 ping 循环：每 `pingInterval` 发 control 帧
   `headers=[{key:"type",value:"ping"}]`、`service=Number(serviceId)`、`SeqID=0`、
   `LogID=0`（:85483-85501）。
2. 收帧按 `method` 分派（:85502-85513）：
   - control：`type=ping` 忽略；`type=pong` 且有 payload → JSON
     `{PingInterval,ReconnectCount,ReconnectInterval,ReconnectNonce}` 更新会话参数
     （:85527-85539）。
   - data：headers 归并为 map 取 `message_id/sum/seq/type/trace_id`；`type !== "event"`
     忽略（MessageType.card 有定义但此路径不处理）；payload 经 DataCache 分片重组
     （见下）后派发。
3. **分片重组 DataCache**（:85202-85259）：`message_id → Array(sum)` 预填 undefined，
   按 `seq` 放 payload；全部非空才合并（按 seq 顺序字节拼接 → UTF-8 → `JSON.parse`）
   并派发，派发即删缓存；**每 10s 清理 10s 前的未完成分片**。
4. 派发后回 **ACK 帧**：复制原帧（SeqID/LogID/service/method 不变），headers 追加
   `{key:"biz_rt", value:处理耗时 ms}`，payload = `JSON {"code":200|"500"}`——
   dispatcher 成功 code=200（有结果时 `data=base64(JSON(result))`），异常 code=500
   （handleEventData :85550-85585；HttpStatusCode :85292-85298）。
5. WS error 只记日志；**WS close → 重连**（:85514-85520）。

### 5.4 重连

1. `generation` 计数防陈旧重连循环（:85393-85481）。
2. 每轮 = 注册（pullConnectConfig）→ 成功才建连 → 成功才挂消息处理；注册/建连失败
   → 重试整轮；**启动失败立即递归重试**。
3. 断线重连：先等 `reconnectNonce × random()` 抖动，再按 `reconnectInterval` 间隔
   重试至多 `reconnectCount` 次（默认 -1 = 无限）（:85439-85479）。
4. 手动 `close()`：generation++ 清计时器（:85663-85690 区间）。

## 5. 验收

1. 离线：`cargo test -p dingtalk-stream` 全绿（纯函数 + 假 WS 集成）。
2. 行为对齐：spec §3 每条在代码内有对齐注释（TS file:line）。
3. 真机（后续）：真实钉钉应用凭据下与 TS 通道并行跑（不同 bot），消息往返、断网重连、
   服务端重投去重各验一轮；通过后才谈 TS 通道退役。
