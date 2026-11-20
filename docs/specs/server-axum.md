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
- 可运行二进制：`MODE_SERVER_PORT=3031 cargo run -p mode-server`。
- 测试 7 条：健康、版本、server-info 形状、未知路由 404 + 3 条 env/投影纯逻辑。

这一版**不接 Host 服务**，只固定 axum 入口、错误面与测试方式；不能替代
`packages/server`，也不改变现有 Web/远控链路。

## 3. 后续切片

1. LAN 只读面（`/api/lan/state`、`peers`）：复用 `crates/lan-*`，写操作仍走现有 Host。
2. `/ws/host`：先接 RPC 帧转发，保持 `desktop-continuous` 与
   `web-remote-replayable` 两种语义（见 W2 spec）。
3. 更新/远程桌面/附件组逐组迁移；每组先补契约对拍，再切换所有权。
4. 全部路由对拍通过后，`packages/server` 才进入退役。

## 4. 边界

- axum 层不持有会话/任务/队列状态；只做鉴权、路由、帧转发与静态资源。
- 错误码与响应形状以现有 TS 契约真源为准，不自行发明。
- WebSocket 改动必须同时验桌面连续链路与手机可重放链路。
