# WSS 中继 MVP（自建 Mode Cloud 传输层）

状态：2026-10-21 实现中。关联：
[lan-remote-public-endpoints.md](./lan-remote-public-endpoints.md)（公网端点与公网优先）、
[mode-cloud-account-directory.md](./mode-cloud-account-directory.md)（账号/设备目录）、
[in-app-remote-desktop.md](./in-app-remote-desktop.md)（远控协议）、
[lan-remote-desktop-native-media.md](./lan-remote-desktop-native-media.md)（媒体面）。

## 目标

让被控端不需要公网 IP / 端口映射，也能被外网主控连接：

```text
主控 ── HTTPS/WSS ──► 中继（fnOS） ◄── 出站 WSS ── 被控端（Mode Desktop/Agent）
                              │
                              └── 转发到被控端本地 45880
```

中继只做**透明 HTTP/WS 转发**，复用现有远控协议；默认公网中继优先，LAN 作为回退
（[lan-remote-public-endpoints.md](./lan-remote-public-endpoints.md)）。

## 非目标（本期明确不做）

- 账号/设备目录已落在 [mode-cloud-account-directory.md](./mode-cloud-account-directory.md)；
  中继转发本身仍不保存任务、会话或业务内容。
- 端到端加密（E2E）：本期中继是**可信自建中继**，能看到明文 HTTP/WS；上行输入/
  剪贴板的 E2E 是上线到不可信中继前的前置。
- P2P/NAT 穿透。
- 跨账号设备发现；内置中继只服务同一部署（端点自动下发见关联 spec）。
- 手机/浏览器远控。

## 产品规则

1. **出站隧道**：被控端启动 `cloud-relay` 隧道客户端，主动连
   `wss://<relay>/_tunnel?tunnelId=<id>&token=<token>`；不要求被控端入站端口。
   Mode Desktop 的 LAN 服务在配置了 `MODE_RELAY_TOKEN` 时自动启动；CLI 仍可手动运行。
2. **公开地址**：中继把 `tunnelId` 映射为
   `https://<relay>/d/<tunnelId>`；被控端在配对响应/`server-info` 里下发
   `publicRelay`，主控自动写入公网端点，**不需要手填完整 URL**。
3. **路径前缀**：主控的 server-info、session POST、信令 WS、媒体 WS 都带
   `/d/<tunnelId>` 前缀；中继剥离前缀后转发到被控端本地根路径。
4. **鉴权**：
   - 隧道连接用 `RELAY_TUNNEL_TOKEN`（设备侧与中继共享的随机串）；
   - 公开路径 `tunnelId` 是 24 字节随机 base64url（高熵、不可枚举）；
   - 主控仍使用现有 LAN 令牌/票据；中继不解析、不改写它们。
5. **可信边界**：本期中继能看到 HTTP/WS 明文（包括键鼠与剪贴板）；只允许部署在
   自己控制的 fnOS/VPS。把中继开放给第三方前必须先做上行 E2E。
6. **限制**：
   - HTTP 请求/响应体上限 4 MiB（远控的 API 都是小包；附件走媒体/其他通道）；
   - WS 单帧上限 16 MiB；
   - HTTP 请求超时 120s（覆盖 90s 同意窗）；WS 打开超时 15s；
   - 隧道心跳 30s；断线后客户端指数退避重连（1s→30s）。
7. **失败语义**：
   - 无对应隧道 → 503；
   - 隧道存在但本地 WS 打开失败 → 502；
   - 隧道断开 → 进行中的 HTTP 502、WS 以 1011 关闭；
   - 超过 body/frame 限制 → 413 / 1009。
8. **不改变现有传输**：LAN/Tailscale/直连公网端点逻辑不变；中继只是新增一个
   `https/wss` 端点（带 path）。

## 状态所有者

| 状态 | 所有者 | 说明 |
| --- | --- | --- |
| 隧道注册表（tunnelId → WS） | `cloud-relay` 服务进程 | 单进程内存；断开即移除 |
| 活跃 stream（HTTP/WS） | `cloud-relay` 服务进程 | streamId 单调递增，连接级 |
| 本地 HTTP/WS 代理 | 隧道客户端进程 | 每个 stream 一条本地连接 |
| 远控会话/同意/票据 | 被控端 Mode Host | 中继不参与、不持久化 |
| 账号/设备目录 | cloud-relay 内 SQLite（`cloud_*`） | 仅 `/v1` 控制面读写；`/d/*` 转发不读 |

中继不保存任务、会话、工作区、令牌或任何业务状态；只保存连接期内存映射。

## 协议

隧道是一条 WSS。控制消息为 JSON text frame；WS 数据为 binary frame。

```text
binary frame: [u32 streamId][u8 kind][payload]
kind: 1 = text message, 2 = binary message
```

控制消息：

```ts
{ type: "hello", version: 1, tunnelId }
{ type: "ready", version: 1, publicPath: "/d/<tunnelId>" }
{ type: "ping", ts } / { type: "pong", ts }
{ type: "http-request", streamId, method, path, headers, bodyBase64? }
{ type: "http-response", streamId, status, headers, bodyBase64? }
{ type: "http-error", streamId, message }
{ type: "ws-open", streamId, path, headers }
{ type: "ws-ready", streamId } / { type: "ws-error", streamId, message }
{ type: "ws-close", streamId, code?, reason? }
```

HTTP 转发：

```text
主控 GET /d/<id>/api/server-info?token=…
  → 中继发 http-request {path:"/api/server-info?token=…"}
  → 隧道客户端请求 http://127.0.0.1:45880/api/server-info?token=…
  → http-response → 中继写回主控
```

WS 转发：

```text
主控升级 /d/<id>/ws/remote-desktop?ticket=…
  → 中继发 ws-open
  → 隧道客户端连 ws://127.0.0.1:45880/ws/remote-desktop?ticket=…
  → ws-ready → 中继完成升级
  → 双向 binary frame 透传（text/binary 标志保留）
```

## 接口与部署

- 新包：`packages/cloud-relay`。
  - `startRelayServer({ host?, port, token, publicBaseUrl, maxHttpBodyBytes?, log? })`
  - `startRelayTunnelClient({ relayUrl, token, tunnelId, localBaseUrl, log? })`
  - CLI：`src/cli-server.ts`、`src/cli-tunnel.ts`。
- 控制面（可选，同进程）：`/v1/devices/enroll`、`/v1/devices`、
  `/v1/devices/:id/revoke`、`/v1/health`；协议见
  [mode-cloud-account-directory.md](./mode-cloud-account-directory.md)。
- 环境变量：
  - `MODE_RELAY_HOST`（默认 `127.0.0.1`）
  - `MODE_RELAY_PORT`（默认 `8787`）
  - `MODE_RELAY_TOKEN`（必填）
  - `MODE_RELAY_PUBLIC_BASE_URL`（例如 `https://relay.example.com`）
  - `MODE_CLOUD_ENROLL_TOKEN`（可选；提供时启用 `/v1` 控制面）
  - `MODE_CLOUD_ACCOUNT_PEPPER`（可选；微信 ID HMAC key，缺省复用 enrollment token）
  - `MODE_CLOUD_DB_PATH`（默认 `/data/cloud.sqlite`）
- 反向代理：Caddy/nginx 把 `https://relay.example.com` 反代到
  `127.0.0.1:8787`，自动处理 WebSocket upgrade；证书可用 DNS-01。
- 内置模式：`packages/desktop` 的 `startLanAccessServer` 在注入 `relay.token` 或
  `MODE_RELAY_TOKEN` 时生成并持久化 `tunnelId`（凭据
  `lan:access:relay-tunnel-id`），自动启动隧道；默认
  `MODE_PUBLIC_RELAY_BASE_URL=https://relay.mymode.cloud`，可用 env 覆盖。
- 主控：配对后自动写 `https://<base>/d/<tunnelId>`，随后按 `公网 → LAN` 探测；
  无需手填公网地址。
- 被控端：Mode Desktop 必须开启「局域网访问」与「远程桌面（应用内）」；内置隧道
  只把本地监听端口转发到中继，不替代这两个开关。未配置中继令牌时保持 LAN-only。
- 不要同时用同一 `tunnelId` 运行 CLI 与 Desktop 内置隧道：中继以最后一次注册为准，
  两者会互相顶替。

## 安全边界

- `tunnelId` 是公开路径能力，必须高熵；泄露后攻击者可到达被控端 HTTP/WS，但仍需
  现有 LAN 令牌/票据才能操作。
- 隧道 token 只用于设备→中继鉴权，不进 URL 日志；中继日志只记 tunnelId 前缀。
- 中继不记录 body、Cookie、Authorization、ticket、mediaTicket。
- 本期不做 E2E：中继（fnOS）能看到明文键鼠/剪贴板；上线到不可信中继前必须补齐。
- `MODE_CLOUD_ENROLL_TOKEN` 是部署级 secret；泄漏可伪造账号/设备注册。控制面只在
  TLS 后面启用，并对 enroll 做限速；不要把 secret 写进镜像或仓库。

## 验收

1. 本地集成测试：假本地服务 + 中继 + 隧道客户端；HTTP GET/POST、WS 文本/二进制
   双向透传；带 `/d/<id>` 前缀；无隧道 503；隧道断开 502。
2. 路径模型：`https://relay/d/<id>` 经过端点解析后，server-info/session/WS 的 URL
   都保留前缀；旧 LAN/公网端点不带 path 时行为不变。
3. 被控端发起 pair（`/d/<id>/api/lan/pair`）能到达本地 45880。
4. `pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed` 与相关测试全绿。
5. 真机：fnOS 上跑中继 + Windows 上跑隧道客户端；外网主控能连接，LAN 内仍直连。
