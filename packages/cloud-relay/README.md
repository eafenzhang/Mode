# @mode/cloud-relay（WSS 中继 MVP）

自建 WSS 中继：被控端主动出站连中继，主控通过 `https://relay/d/<tunnelId>` 访问，
不需要被控端公网 IP 或端口映射。协议与边界见
[docs/specs/wss-relay-mvp.md](../../docs/specs/wss-relay-mvp.md)。

> 本期是**可信自建中继**：中继能看到明文 HTTP/WS（包括键鼠与剪贴板）。只在自己
> 控制的 fnOS/VPS 上使用；开放给第三方前必须先做上行 E2E。

## 1. 启动中继（fnOS/VPS）

fnOS 上推荐用部署脚本（自动生成令牌、复制文件、启动容器并做健康检查）：

```bash
./scripts/deploy-fnos.sh --public-base-url 'https://relay.example.com'
```

脚本只启动服务端，不涉及被控端和 TLS；令牌写入安装目录的 `.env`。

手动方式：

```bash
MODE_RELAY_TOKEN='<长随机串>' \
MODE_RELAY_HOST=0.0.0.0 \
MODE_RELAY_PORT=8787 \
MODE_RELAY_PUBLIC_BASE_URL='https://relay.example.com' \
pnpm --filter @mode/cloud-relay exec tsx src/cli-server.ts
```

- 只监听 `127.0.0.1:8787` 时，交给 Caddy/nginx 反代（推荐）。
- 直接对公网监听时，建议只开 443/8443，并用防火墙限制来源。
- fnOS Docker 方式：

```bash
cd packages/cloud-relay
MODE_RELAY_TOKEN='<长随机串>' \
MODE_RELAY_PUBLIC_BASE_URL='https://relay.example.com' \
docker compose up -d --build
```

- 国内网络如果拉不到 `node:24-alpine`，可指定可用镜像源：

```bash
MODE_RELAY_NODE_IMAGE='docker.m.daocloud.io/library/node:24-alpine' \
MODE_RELAY_NPM_REGISTRY='https://registry.npmmirror.com' \
./scripts/deploy-fnos.sh --public-base-url 'https://relay.example.com'
```

### Mode Cloud 控制面（可选）

同一容器内启用账号/设备目录
（[docs/specs/mode-cloud-account-directory.md](../../docs/specs/mode-cloud-account-directory.md)）：

```bash
MODE_CLOUD_ENROLL_TOKEN='<部署级 secret>' \
MODE_CLOUD_ACCOUNT_PEPPER='<独立随机串>' \
MODE_CLOUD_DATA_DIR='./data' \
./scripts/deploy-fnos.sh --public-base-url 'https://relay.example.com'
```

- 端点：`/v1/devices/enroll`、`/v1/devices`、`/v1/devices/:id/revoke`、
  `/v1/auth/start`、`/v1/auth/poll`、`/v1/health`；
- SQLite 落在 `./data/cloud.sqlite`，需要备份；
- `MODE_CLOUD_ENROLL_TOKEN` 留空 = 关闭控制面，只保留中继；
- 同一个 secret 也要配置到各 Desktop 的 `MODE_CLOUD_ENROLL_TOKEN`。

### 云端微信 Bot 登录（换机零配置）

给云端配置一个**独立**的 iLink bot token（不要与桌面 Bot 共用，否则会互相抢
`getupdates` 消息）：

```bash
MODE_CLOUD_WECHAT_TOKEN='<独立 iLink bot token>' \
MODE_CLOUD_WECHAT_ALLOWED_USER='<你的微信 ID，可选>' \
./scripts/deploy-fnos.sh --public-base-url 'https://relay.mymode.cloud'
```

- `MODE_CLOUD_WECHAT_ALLOWED_USER` 留空 = TOFU：第一个用有效登录码的微信用户成为账号锚点。
- 新电脑安装后直接「使用微信登录」：显示 6 位码 → 发给云端 Bot → 自动获取
  device token / relay token；不需要部署 secret 或 `cloud-deployment.json`。
- 未配置 token 时 `/v1/auth/*` 返回 503，旧 enrollment secret 路径仍可用。

- 反代示例（Caddy 自动处理 WebSocket upgrade）：

```caddyfile
relay.example.com {
  reverse_proxy 127.0.0.1:8787
}
```

证书用 DNS-01 签发，不依赖 80 端口。

## 2. 启动被控端隧道

Mode Desktop 已内置自动隧道：开启「局域网访问」且注入 `MODE_RELAY_TOKEN` 时，
Host 会自动生成并持久化 `tunnelId`、启动隧道，并在配对响应/`server-info` 中下发
`publicRelay`；主控据此自动写入 `https://<base>/d/<tunnelId>`，不需要手填。
`MODE_PUBLIC_RELAY_BASE_URL` 默认 `https://relay.mymode.cloud`，可用 env 覆盖。
Desktop 也支持 `cloud-deployment.json` 一次写入 `cloudBaseUrl` / `enrollToken` /
`relayToken`（权限 0600、勿提交），之后不再逐机设置环境变量。
未配置令牌时保持 LAN-only。不要与使用同一 `tunnelId` 的 CLI 同时运行（中继以最后
一次注册为准）。

手动 CLI 方式（调试/独立部署）：

前置：被控端 Mode Desktop 已开启「局域网访问」与「远程桌面（应用内）」；隧道客户端
只是把本机 45880 转发到中继，不替代这两个开关。Windows 侧需要 Node 24 + pnpm
（仓库检出即可）。

在 Windows 被控机（或能访问 `127.0.0.1:45880` 的同机/同 LAN 机器）运行：

```bash
MODE_RELAY_TOKEN='<同一个长随机串>' \
pnpm --filter @mode/cloud-relay exec tsx src/cli-tunnel.ts \
  --relay 'wss://relay.example.com/_tunnel' \
  --local 'http://127.0.0.1:45880' \
  --generate
```

输出：

```text
[relay-tunnel] tunnelId=<高熵随机串>
[relay-tunnel] public base: https://relay.example.com/d/<高熵随机串>
```

内置模式下这个地址会随配对自动下发；手动 CLI 调试时，把 `public base` 填到 Mode
远程控制页的设备「公网地址」，或直接在「公网设备」里填这个 URL + 对端 6 位配对码。

隧道客户端会断线重连（1s→30s 退避）；`--tunnel-id` 可固定路径，`--generate` 每次
生成新的高熵路径。

Windows 自启动：先用任务计划程序或 NSSM 把上面的命令包成服务（工作目录设为仓库根，
`pnpm` 在 PATH 中）。MVP 先手动运行；后续再做安装器/托盘常驻。

如果 fnOS 能访问 Windows 的 `192.168.x.x:45880`，也可以把隧道客户端跑在 fnOS 上，
把 `--local` 指向 Windows 的局域网地址；这样 Windows 侧不需要 Node，只需放行
局域网入站 45880。

## 3. 部署建议

- 中继与被控端之间的隧道由被控端主动发起；被控端不需要入站端口。
- `tunnelId` 是公开路径能力，必须高熵；泄露后攻击者能到达被控端 HTTP/WS，但仍需
  现有 LAN 令牌/票据。
- 中继只保存连接期内存映射，不保存任务/会话/令牌。
- 中继日志不打印 body、Cookie、Authorization、ticket；只打印 tunnelId 前缀。
- 家庭 fnOS：上行带宽是媒体瓶颈；建议先 1 路 1080p 均衡（约 5 Mbps）验证。

## 4. 测试

```bash
pnpm --filter @mode/cloud-relay test
```

覆盖：HTTP GET/POST、WS 文本/二进制、`/d/<tunnelId>` 路径前缀、无隧道 503、
隧道断开 503、HTTP body 上限 413、健康检查。
