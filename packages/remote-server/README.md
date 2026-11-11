# @mode/remote-server

远程控制 v3 的账号、设备目录、membership、Control 与 Relay 服务。
规范：[docs/specs/remote-control-v3.md](../../docs/specs/remote-control-v3.md)。

## 运行

```sh
MODE_REMOTE_SECRET="<高熵随机串>" \
MODE_REMOTE_PORT=45890 \
MODE_REMOTE_DB=./data/remote-v3.sqlite \
pnpm --filter @mode/remote-server dev
```

生产可直接由本进程终止 TLS，或继续放在反向代理后面。
可选环境变量：`MODE_REMOTE_PEPPER`、`MODE_REMOTE_PUBLIC_URL`、
`MODE_REMOTE_TLS_KEY_FILE` + `MODE_REMOTE_TLS_CERT_FILE`（同时提供时进程用 HTTPS/WSS）、
`MODE_REMOTE_METRICS_TOKEN`（提供时暴露 Prometheus `/metrics`）、
`MODE_GITHUB_CLIENT_ID`（提供时暴露浏览器 GitHub Device Flow 代理）、
`MODE_GITHUB_API_BASE_URL`、`MODE_REMOTE_ALLOWED_ORIGINS`（逗号分隔的浏览器 Origin；
同时用于 Control/Relay WS 与 `/api/v1/*` 的 CORS 白名单。原生客户端不带 Origin，
不受影响；未配置时不发 CORS 头，仅同源页面可用）。

反向代理 / fnOS 网关：服务地址可带子路径（如 `https://nas/app/mode`），API 与
Control/Relay WS 都会保留该前缀。`MODE_REMOTE_PUBLIC_URL` 既可写完整 origin，
也可只写网关前缀（如 `/app/mode`）：后者下发的 Relay 端点是相对路径，由客户端按自己的
服务地址解析，适合网关域名不固定的部署。

## 端点

- `POST /api/v1/auth/github`：GitHub access token 换 Web 会话。
- `POST /api/v1/auth/github/device/start` / `.../poll`：浏览器 Device Flow 代理；
  GitHub access token 只在服务端使用，轮询成功后直接返回 Web 会话。
- `POST /api/v1/devices`：Web 会话注册 Host/Client 设备（X25519 公钥）并签发
  access + refresh。
- `GET /api/v1/devices`：设备凭证读取同账号异角色设备，只返回 descriptor 与在线状态。
- `POST /api/v1/connections`：Client 创建连接授权。
- `GET /ws/v1/connect`：Control 平面（hello / connect / secure.handshake / relay）。
- `GET /ws/v1/relay`：独立 Relay data WebSocket（签名 ticket + 密文转发）。
- `GET /health`：进程存活；`GET /ready`：只读 DB 探针，未就绪返回 503。
- `GET /metrics`：Prometheus 文本计数（Control 连接、逻辑连接、Relay 帧/字节、API 请求）；
  需要 `Authorization: Bearer <MODE_REMOTE_METRICS_TOKEN>` 或 `x-metrics-token`。

数据库 schema 使用 `PRAGMA user_version` 标记版本；启动时自动执行幂等迁移，版本高于
当前代码会拒绝启动。

## 安全边界

- 不保存、不返回被控端本机 API 令牌。
- Relay 不解密、不解析、不落盘、不记录 `ciphertext`。
- 设备撤销立即删除 token、撤销 membership、关闭相关 Control/Relay 连接。
- 不兼容 v2 的 `/d/<tunnelId>`、设备 `deviceToken` 或 LAN Bearer 语义。

## 测试

```sh
pnpm --filter @mode/remote-core test
pnpm --filter @mode/remote-server test
```
