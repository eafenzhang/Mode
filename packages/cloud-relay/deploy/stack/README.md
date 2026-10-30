# Mode 单容器部署（Mode Cloud + Mode Go relay + 官网）

把三个常驻服务装进**一个容器**（`mode-stack`），宿主端口映射与 Cloudflare Tunnel
路由保持不变，因此不需要改 ingress：

| 服务 | 容器内端口 | 宿主映射 | 对外 |
| --- | --- | --- | --- |
| Mode Cloud（控制面/中继/门户） | 8787 | `127.0.0.1:8787` | `relay.mymode.cloud` |
| Mode Go relay | 8788 | `0.0.0.0:8788` | `agentgo.mymode.cloud`（按现有 ingress） |
| 官网静态站 | 80 | `127.0.0.1:8095` | `mymode.cloud` |

三个进程由 `start.sh` 同容器拉起；任一退出即整体退出，由 compose
`restart: unless-stopped` 重启，避免“容器活着但某个服务已死”。

## 目录约定（部署机）

```text
~/mode-stack/
├── Dockerfile            # 本模板
├── start.sh              # 本模板
├── docker-compose.yml    # 本模板
├── .env                  # 合并后的环境变量（0600，勿提交）
├── cloud-relay/          # 从仓库 packages/cloud-relay 复制的 package.json + src
├── data/                 # Mode Cloud SQLite（从旧部署迁移）
├── agent-go/             # 可选：亦可直接用宿主机绝对路径挂载 Mode Go 源码
└── ...
```

`docker-compose.yml` 通过环境变量挂载外部源码/站点目录：

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `MODE_GO_SOURCE_DIR` | `/vol1/1000/agent-go-relay` | Mode Go relay 源码目录（含 `src/server.mjs` 与 `node_modules`） |
| `MODE_SITE_DIR` | `/vol1/docker/mymode-site` | 官网静态文件目录（需可写：版本号刷新） |
| `MODE_SITE_SERVER` | `/vol1/docker/mymode-site-server.cjs` | 官网静态服务脚本 |

`docker-compose.yml` 另外固定 `MODE_CLOUD_DB_PATH=/data/cloud.sqlite`。旧部署把该默认值
写在 compose 里而没有进 `.env`，单容器部署若沿用 cli-server 的相对默认值
`./data/cloud.sqlite`，会在容器可写层建出一个空库，表现为 `/v1/health` 的
`accounts/devices` 归零（数据其实还在挂载卷里）。

## 部署

```bash
# 1) 组装构建上下文
mkdir -p ~/mode-stack/cloud-relay
cp <repo>/packages/cloud-relay/package.json ~/mode-stack/cloud-relay/
cp -R <repo>/packages/cloud-relay/src ~/mode-stack/cloud-relay/
cp <repo>/packages/cloud-relay/deploy/stack/{Dockerfile,start.sh,docker-compose.yml} ~/mode-stack/
chmod 755 ~/mode-stack/start.sh

# 2) 环境变量：沿用旧 cloud-relay 的 .env，并补 Mode Go 的变量（PORT=8788、
#    HOST、ADMIN_TOKEN 等）；官网用默认 SITE_PORT=80 / SITE_ROOT=/srv/public。
#    从旧容器导出 Mode Go 变量（不打印值）：
docker inspect agent-go-relay --format '{{range .Config.Env}}{{println .}}{{end}}' \
  | grep -E '^(PORT|HOST|ADMIN_TOKEN|PAIR_TTL_MS|MAX_PAIRS_PER_AGENT|MAX_FRAME_BYTES|PAIR_RATE_LIMIT|TRUST_PROXY|LOG_LEVEL|SESSION_RESUME_TTL_MS)=' \
  >> ~/mode-stack/.env
chmod 600 ~/mode-stack/.env

# 3) 迁移 SQLite（保留旧目录用于回滚）
mkdir -p ~/mode-stack/data
docker run --rm -v ~/mode-cloud-relay/data:/from:ro -v ~/mode-stack/data:/to \
  node:24-alpine sh -c 'cp -a /from/. /to/'

# 4) 构建并切换（先停旧容器释放端口）
docker compose -f ~/mode-stack/docker-compose.yml build
docker stop mymode-www agent-go-relay
# 旧容器保留用于回滚，但必须去掉自启：agent-go-relay 原为 restart=always，
# NAS 重启会抢回 8788 端口，导致 mode-stack 起不来。
docker update --restart=no mymode-www agent-go-relay
docker compose -f ~/mode-cloud-relay/docker-compose.yml down
docker compose -f ~/mode-stack/docker-compose.yml up -d

# 5) 验证
curl -fsS http://127.0.0.1:8787/v1/health
curl -fsS -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8787/
curl -fsS -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8095/
curl -fsS --max-time 20 https://relay.mymode.cloud/v1/health
```

## 回滚

```bash
docker compose -f ~/mode-stack/docker-compose.yml down
docker start mymode-www agent-go-relay
docker compose -f ~/mode-cloud-relay/docker-compose.yml up -d
```

## 边界

- 三个进程共享容器网络与文件系统；容器内 8787/8788/80 互不冲突。
- Mode Go 源码与官网文件通过宿主机绑定挂载，不复制进镜像；更新它们不需要重建镜像。
- 容器以 root 运行（node 官方镜像默认），绑定 80 端口需要该权限。
- 单点故障：任一进程崩溃会整体重启（三个服务一起闪断，通常 < 3 秒）。
- Cloudflare Tunnel ingress、TLS、域名均不变。
