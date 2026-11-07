# Spec：LAN 配对 HTTP 处理器（Rust `lan-http`）

状态：2026-11-05 第一版（传输无关的 `/api/lan/pair`）。
真源：`packages/server/src/http.ts` 的 `/api/lan/pair` 与 Cookie/令牌读取。

## 1. 范围

| Rust 入口 | TS 对应 |
| --- | --- |
| `LanPairHttp::handle_pair` | `POST /api/lan/pair` 的 body 校验、限速与兑换 |
| `LanPairHttp::get_state` / `create_pair_code` / `remove_client` / `reset_tokens` | `/api/lan/state` 与三个动作端点 |
| `LanAccessView` | 传输层写入的 enabled/port/addresses/会话数/中继地址 |
| `PairHttpResponse` | 状态码 + JSON body + `Set-Cookie` |
| `serve` | hyper 1.x HTTP/1.1 服务（配对 + 管理端点） |
| `presented_token` | `readPresentedLiteToken`（Bearer / `?token=` / 两种 Cookie） |
| `PAIRING_ATTEMPT_LIMIT` / `PAIRING_ATTEMPT_WINDOW` | 30 次 / 60s |

**不做**：TLS、WebSocket、公网中继、远程控制端点。
HTTP/1.1 传输由 `serve` 提供；宿主用 `set_view` 同步监听状态。

## 2. 行为

- 请求体：JSON 对象；`code` trim 后 4..=32；`label` 可选、trim 后 ≤64。
- 失败口径：
  - 非法 JSON / 非对象 → `400 {"error":"Invalid JSON body"}`；
  - 字段非法 → `400 {"error":"Invalid request body: <field>"}`；
  - 限速命中 → `429 {"error":"Too many pairing attempts"}`；
  - 码错/过期/已被消费 → `401 {"error":"Invalid or expired pair code"}`
    （不区分原因，避免探测信号）；
  - 落盘失败 → `500 {"error":"Failed to persist pairing result"}`。
- 成功：`200` + 配对结果 JSON（`token/serverId/name?`），并返回
  `mode_lite_token=<token>; Path=/; HttpOnly; SameSite=Lax`。
- 令牌提取优先级：`?token=` → `Authorization: Bearer` → `mode_lite_token` Cookie →
  `zcode_lite_token` 旧 Cookie。
- 限速在解析 body 之前计数（与 TS 一致）。
- 管理端点：
  - `get_state` 返回 `{enabled,port,addresses,pairCode,clients,activeWebSessions,publicRelayUrl}`；
  - `create_pair_code` / `remove_client` / `reset_tokens` 在监听未开启时返回
    `Err("LAN access is not enabled")`（由传输层映射为错误响应）；
  - 三个动作成功后都返回最新状态快照（与 TS 返回 `LanAccessState` 一致）；
  - 落盘失败返回 `Err("Failed to persist LAN clients")`。
- 传输路由与鉴权：
  - `GET /api/lan/state`、`POST /api/lan/pair-code`、`DELETE /api/lan/clients/:id`、
    `POST /api/lan/reset` 需要有效长期令牌（`Authorization: Bearer`、`?token=` 或 Cookie）；
  - 未授权 `401 {"error":"Unauthorized"}`；管理动作被拒绝（监听未开启）→ `409`；
  - 未知路径 `404`。

## 3. 差异

1. hyper server 的 `httpdate` 依赖在离线环境不可获取；仓库用 `crates/httpdate-shim`
   通过 `[patch.crates-io]` 提供 IMF-fixdate 格式化/解析（仅 Date 头需要）。
2. 请求体大小上限由 hyper/宿主负责；处理器本身不做无界缓冲。
3. 只实现 HTTP/1.1，无 keep-alive 调优与 TLS；公网中继另属服务 crate。

## 4. 验收

- `cargo test -p lan-http`：12 条（成功 + Cookie、错误码不泄露原因、非法 JSON、
  限速 429、令牌优先级、旧 Cookie、记录序列化、状态快照、未开启拒绝、撤销/重置、
  hyper 端到端配对、管理端点鉴权）。
- 与 TS 的端到端对拍在传输接线后随设置页验收补齐。
