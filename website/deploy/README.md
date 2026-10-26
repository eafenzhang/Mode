# mymode.cloud 部署

Mode 官网（`website/public/`）部署在 fnOS NAS 上，经 Cloudflare Tunnel 对外服务。

## 链路

```
浏览器
  → Cloudflare 边缘（HTTPS 终结，proxied）
  → cloudflared 隧道（fnOS「Tunnel」应用，token 方式远程托管，ingress 在云端配置）
  → 宿主 127.0.0.1:8095
  → docker 容器 mymode-www（node:24-alpine 运行 nas-static.js）
  → /vol1/docker/mymode-site/（站点文件，由 deploy.sh 同步）
```

域名的 DNS 与隧道 ingress 的所有者是 Cloudflare；站点文件唯一来源是仓库 `website/public/`，
NAS 目录只是运行时副本。NAS 拉取不了 Docker Hub，因此静态服务用 NAS 上已有的
`node:24-alpine` 而不是 nginx。

## 日常更新（改完站点后）

```bash
./website/deploy/deploy.sh        # tar 流同步文件到 NAS + 确保容器在跑
```

## 变更路由 / DNS（很少需要）

`setup-cloudflare.py` 幂等，在 NAS 上执行（token 从 tunnel 应用配置读取，不出 NAS）：

```bash
scp website/deploy/setup-cloudflare.py eafen@100.104.219.122:/tmp/
ssh eafen@100.104.219.122 python3 /tmp/setup-cloudflare.py
```

当前路由（`relay` / `agentgo` / `hub-cmd` / `gashub` 是业务子域，脚本不会触碰）：

| hostname            | service              |
| ------------------- | -------------------- |
| mymode.cloud        | http://127.0.0.1:8095 |
| www.mymode.cloud    | http://127.0.0.1:8095 |
| relay.mymode.cloud  | http://127.0.0.1:8787 |
| agentgo.mymode.cloud| http://127.0.0.1:8788 |
| hub-cmd / gashub    | http://127.0.0.1:8080 |
| (catch-all)         | http_status:404      |

## 凭证边界

- Cloudflare API token 存放在 NAS `/vol1/@appconf/com.dustinky.tunnel/config.json`，
  只在 NAS 上被读取，不进入仓库、不落本机。
- SSH 用 `~/.ssh/mode-fnos-deploy` 密钥连 `eafen@100.104.219.122`（Tailscale）。

## 诊断

```bash
ssh eafen@100.104.219.122 docker ps --filter name=mymode-www        # 容器状态
ssh eafen@100.104.219.122 curl -s http://127.0.0.1:8095/healthz     # 源站健康
curl -s -o /dev/null -w '%{http_code}' https://mymode.cloud/healthz # 全链路健康
```
