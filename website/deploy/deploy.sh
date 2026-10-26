#!/usr/bin/env bash
# 同步官网静态文件到 fnOS NAS，并确保静态服务容器在运行。
# 用法：在仓库根目录执行 ./website/deploy/deploy.sh
#
# 说明：NAS 无法拉取 Docker Hub（nginx:alpine 不可得），静态服务用 NAS 上已有的
# node:24-alpine 运行 website/deploy/nas-static.js，端口映射 127.0.0.1:8095 -> 容器 80。
# 隧道/DNS 配置见 setup-cloudflare.py（只在变更路由时需要）。
set -euo pipefail
cd "$(dirname "$0")/../.."

NAS_HOST="${MODE_NAS_HOST:-eafen@100.104.219.122}"
NAS_KEY="${MODE_NAS_KEY:-$HOME/.ssh/mode-fnos-deploy}"
SITE_DIR="${MODE_SITE_DIR:-/vol1/docker/mymode-site}"
SERVER_JS="${MODE_SITE_SERVER_JS:-/vol1/docker/mymode-site-server.js}"
PORT="${MODE_SITE_PORT:-8095}"
CONTAINER="${MODE_SITE_CONTAINER:-mymode-www}"

ssh_cmd() { ssh -i "$NAS_KEY" -o BatchMode=yes "$NAS_HOST" "$@"; }

ssh_cmd "mkdir -p '$SITE_DIR'"
tar czf - -C website/public . | ssh_cmd "tar xzf - -C '$SITE_DIR'"
scp -q -i "$NAS_KEY" -o BatchMode=yes website/deploy/nas-static.js "$NAS_HOST:$SERVER_JS"

ssh_cmd "
  if docker ps -a --format '{{.Names}}' | grep -qx '$CONTAINER'; then
    docker start '$CONTAINER' >/dev/null
  else
    docker run -d --name '$CONTAINER' --restart unless-stopped \
      -p 127.0.0.1:${PORT}:80 \
      -v '${SITE_DIR}:/srv/public:ro' \
      -v '${SERVER_JS}:/srv/server.js:ro' \
      node:24-alpine node /srv/server.js >/dev/null
  fi
  docker ps --filter name='^${CONTAINER}$' --format '{{.Names}} | {{.Status}} | {{.Ports}}'
  sleep 1
  curl -sf http://127.0.0.1:${PORT}/healthz && echo ' <- healthz ok'
"

echo "[完成] 静态文件已同步到 ${NAS_HOST}:${SITE_DIR}，容器 ${CONTAINER} 监听 127.0.0.1:${PORT}"
