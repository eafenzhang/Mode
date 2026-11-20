# Web/远程服务端改 axum（LT-9）

状态：v1（2026-11-21，第一段纵切）。上位：`docs/plans/2026-10-29-full-rust-rewrite.md` §W5。
现有真源：`packages/server`（Hono + Node Host 服务面）；本 crate 只替换传输层，
业务状态仍归 Host 服务，不在 axum 里复制。

## 1. 路由面盘点（`packages/server/src/http.ts`）

| 组 | 代表路由 | 迁移顺序 |
| --- | --- | --- |
| 诊断/元信息 | `GET /api/server-info`、`POST /api/rpc-host-capability` | 1 |
| LAN | `/api/lan/*`（state/enable/pair-code/clients/reset/peers） | 2 |
| 应用更新 | `/api/app-update/state|check|download|apply` | 3 |
| 远程环境/桌面 | `/api/remote-environments/*`、`/api/remote-desktop/*` | 4 |
| 附件/日志 | `/api/attachments/temp-text`、`/api/logs/export` | 5 |
| WebSocket | `/ws/host`、`/ws/remote-desktop`、`/ws/remote-desktop-media` | 6（与 W2 流控一起验） |

## 2. 第一段纵切（已交付）

`crates/server`：

- `router(config)`：`GET /healthz`、`GET /api/version`、`GET /api/server-info`；
  `router_from_env()` 按 `MODE_SERVER_*` 组装。
- `server_info`：`ServerRemoteInfo` 投影（camelCase、可选能力缺键、auth/workspace
  环境口径与 TS `createServerInfo` 对齐）。
- `lan`：LAN 六端点全部桥接 `crates/lan-http`（`management_request` 传输无关管理面 +
  `handle_pair` 配对端点透传 Set-Cookie；`set_enabled` 更新监听视图并清 pairCode，
  凭据持久化仍属宿主待接 credential-store）。
- 可运行二进制：`MODE_SERVER_PORT=3031 cargo run -p mode-server`。
- 测试 11 条：健康、版本、server-info 形状、404 + LAN 鉴权/state/pair-code/reset/
  删除即吊销/pair 发令牌与 Set-Cookie/enable 开关 + 3 条 env/投影纯逻辑。

这一版**不接 Host 服务**，只固定 axum 入口、错误面与测试方式；不能替代
`packages/server`，也不改变现有 Web/远控链路。

## 3. 后续切片

1. LAN 凭据装配已接（LT-15 P2 切三，2026-11-21）：`lan_pairing_from_store(data_root)`
   —— LAN 配对凭据落 `{data_root}/v2/credentials.json`（与桌面壳 CredentialChannel
   同一文件与加密口径）；生产随机源 uuid v4（token 两个 uuid 拼 32 字节，
   256bit 与 TS randomBytes(32) 对齐）；`main.rs` 升级为完整装配（LAN 管理面 +
   `/ws/host`），凭据库不可用时降级省略 LAN 路由并留日志。
   （`/api/lan/state`、`peers`）：复用 `crates/lan-*`，写操作仍走现有 Host。
2. `/ws/host` 首切已接（LT-15 P2 切二，2026-11-21）：capability 签发
   （`POST /api/rpc-host-capability`，30s TTL 一次性、先删后比防重放）+ WS 升级 +
   每连接一个 `rpc_host::ChannelServer`（TS `new ChannelServer(protocol, "server")`
   同款 deferInit=false）+ `host_channels::assembly` 全频道装配。差异：LAN 客户端
   录制（resolveLanClientByToken/workspace 记录）依赖 Node Host 录制面未迁；
   mode-agent 连接域与 agent 频道依赖 Agent 控制面，server-mode 暂不注册
   （`agent_control: None`，四个频道不可构造留痕跳过）。`desktop-continuous` 与
   `web-remote-replayable` 的 mode-agent 连接语义随 agent 频道接线补验。
   capability 格式差异：TS `randomBytes(32).toString("base64url")`（256bit），
   Rust uuid v4×2 拼接（244bit，无 rand/getrandom 依赖）；header 名与消费语义一致。
   LAN「最近打开目录」录制已接（LT-15 P2 切四，2026-11-21）：升级时
   query `token` → cookie `mode_lite_token`/legacy 兜底 →
   `resolve_client_by_token`（touch lastUsedAt）解析 clientId；连接上每个
   Promise/EventListen 请求帧的第一参数对象顶层 `workspacePath`（path 字段
   不录，PromiseCancel 不解析）→ `record_client_workspace_for_client`
   （同路径去重 + credential-store 落盘）；无 lan_pairing 装配时行为不变
   （TS resolveLanClientByToken 未提供同口径）。
3. 更新/远程桌面/附件组逐组迁移；每组先补契约对拍，再切换所有权。
4. 全部路由对拍通过后，`packages/server` 才进入退役。

## 4. 边界

- axum 层不持有会话/任务/队列状态；只做鉴权、路由、帧转发与静态资源。
- 错误码与响应形状以现有 TS 契约真源为准，不自行发明。
- WebSocket 改动必须同时验桌面连续链路与手机可重放链路。
