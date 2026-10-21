#!/usr/bin/env bash
# 在 fnOS（或其他 Linux NAS）上部署 WSS 中继服务端。
#
# 只部署中继本身：不启动被控端隧道、不改 Mode Desktop、不配置 TLS 公网入口。
# 用法：
#   ./scripts/deploy-fnos.sh --public-base-url https://relay.example.com
# 可选：
#   --dir <目录>              安装目录，默认 $HOME/mode-cloud-relay
#   --public-base-url <URL>   对外访问地址，必填
#   --port <端口>             宿主端口，默认 8787
#   --bind <地址>             宿主绑定地址，默认 127.0.0.1
#   --token <令牌>            固定中继令牌；默认沿用已有 .env 或自动生成
#   --no-build                不重新构建镜像
set -euo pipefail

PACKAGE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INSTALL_DIR="${HOME}/mode-cloud-relay"
PUBLIC_BASE_URL="${MODE_RELAY_PUBLIC_BASE_URL:-}"
TOKEN="${MODE_RELAY_TOKEN:-}"
PORT="${MODE_RELAY_PORT:-8787}"
BIND="${MODE_RELAY_BIND:-127.0.0.1}"
NODE_IMAGE="${MODE_RELAY_NODE_IMAGE:-}"
NPM_REGISTRY="${MODE_RELAY_NPM_REGISTRY:-}"
CLOUD_ENROLL_TOKEN="${MODE_CLOUD_ENROLL_TOKEN:-}"
CLOUD_ACCOUNT_PEPPER="${MODE_CLOUD_ACCOUNT_PEPPER:-}"
CLOUD_DATA_DIR="${MODE_CLOUD_DATA_DIR:-}"
CLOUD_WECHAT_TOKEN="${MODE_CLOUD_WECHAT_TOKEN:-}"
CLOUD_WECHAT_ALLOWED_USER="${MODE_CLOUD_WECHAT_ALLOWED_USER:-}"
CLOUD_WECHAT_BASE_URL="${MODE_CLOUD_WECHAT_BASE_URL:-}"
BUILD_FLAG="--build"

usage() {
  sed -n '2,13p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dir) INSTALL_DIR="$2"; shift 2 ;;
    --public-base-url) PUBLIC_BASE_URL="$2"; shift 2 ;;
    --port) PORT="$2"; shift 2 ;;
    --bind) BIND="$2"; shift 2 ;;
    --token) TOKEN="$2"; shift 2 ;;
    --no-build) BUILD_FLAG=""; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "未知参数：$1" >&2; usage >&2; exit 1 ;;
  esac
done

if [[ -z "$PUBLIC_BASE_URL" ]]; then
  echo "缺少 --public-base-url（例如 https://relay.example.com）" >&2
  exit 1
fi
if ! command -v docker >/dev/null 2>&1; then
  echo "未找到 docker。请先在 fnOS 应用中心安装 Docker。" >&2
  exit 1
fi

if docker compose version >/dev/null 2>&1; then
  COMPOSE=(docker compose)
elif command -v docker-compose >/dev/null 2>&1; then
  COMPOSE=(docker-compose)
else
  echo "未找到 docker compose（v2 或独立版）。" >&2
  exit 1
fi

if ! docker info >/dev/null 2>&1; then
  echo "当前用户没有 Docker 权限。请用 sudo 运行，或把用户加入 docker 组后重新登录。" >&2
  exit 1
fi

mkdir -p "$INSTALL_DIR"
mkdir -p "${CLOUD_DATA_DIR:-$INSTALL_DIR/data}"
install -m 0644 "$PACKAGE_DIR/Dockerfile" "$INSTALL_DIR/Dockerfile"
install -m 0644 "$PACKAGE_DIR/docker-compose.yml" "$INSTALL_DIR/docker-compose.yml"
install -m 0644 "$PACKAGE_DIR/package.json" "$INSTALL_DIR/package.json"
rm -rf "$INSTALL_DIR/src"
cp -R "$PACKAGE_DIR/src" "$INSTALL_DIR/src"

if [[ -f "$INSTALL_DIR/.env" ]]; then
  if [[ -z "$TOKEN" ]]; then
    TOKEN="$(grep -E '^MODE_RELAY_TOKEN=' "$INSTALL_DIR/.env" | head -1 | cut -d= -f2- || true)"
  fi
  if [[ -z "$NODE_IMAGE" ]]; then
    NODE_IMAGE="$(grep -E '^MODE_RELAY_NODE_IMAGE=' "$INSTALL_DIR/.env" | head -1 | cut -d= -f2- || true)"
  fi
  if [[ -z "$NPM_REGISTRY" ]]; then
    NPM_REGISTRY="$(grep -E '^MODE_RELAY_NPM_REGISTRY=' "$INSTALL_DIR/.env" | head -1 | cut -d= -f2- || true)"
  fi
  if [[ -z "$CLOUD_ENROLL_TOKEN" ]]; then
    CLOUD_ENROLL_TOKEN="$(grep -E '^MODE_CLOUD_ENROLL_TOKEN=' "$INSTALL_DIR/.env" | head -1 | cut -d= -f2- || true)"
  fi
  if [[ -z "$CLOUD_ACCOUNT_PEPPER" ]]; then
    CLOUD_ACCOUNT_PEPPER="$(grep -E '^MODE_CLOUD_ACCOUNT_PEPPER=' "$INSTALL_DIR/.env" | head -1 | cut -d= -f2- || true)"
  fi
  if [[ -z "$CLOUD_DATA_DIR" ]]; then
    CLOUD_DATA_DIR="$(grep -E '^MODE_CLOUD_DATA_DIR=' "$INSTALL_DIR/.env" | head -1 | cut -d= -f2- || true)"
  fi
  if [[ -z "$CLOUD_WECHAT_TOKEN" ]]; then
    CLOUD_WECHAT_TOKEN="$(grep -E '^MODE_CLOUD_WECHAT_TOKEN=' "$INSTALL_DIR/.env" | head -1 | cut -d= -f2- || true)"
  fi
  if [[ -z "$CLOUD_WECHAT_ALLOWED_USER" ]]; then
    CLOUD_WECHAT_ALLOWED_USER="$(grep -E '^MODE_CLOUD_WECHAT_ALLOWED_USER=' "$INSTALL_DIR/.env" | head -1 | cut -d= -f2- || true)"
  fi
  if [[ -z "$CLOUD_WECHAT_BASE_URL" ]]; then
    CLOUD_WECHAT_BASE_URL="$(grep -E '^MODE_CLOUD_WECHAT_BASE_URL=' "$INSTALL_DIR/.env" | head -1 | cut -d= -f2- || true)"
  fi
fi
if [[ -z "$TOKEN" ]]; then
  if command -v openssl >/dev/null 2>&1; then
    TOKEN="$(openssl rand -hex 32)"
  else
    TOKEN="$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')"
  fi
fi

umask 077
{
  echo "MODE_RELAY_TOKEN=${TOKEN}"
  echo "MODE_RELAY_PUBLIC_BASE_URL=${PUBLIC_BASE_URL}"
  echo "MODE_RELAY_PORT=${PORT}"
  echo "MODE_RELAY_BIND=${BIND}"
  if [[ -n "$NODE_IMAGE" ]]; then
    echo "MODE_RELAY_NODE_IMAGE=${NODE_IMAGE}"
  fi
  if [[ -n "$NPM_REGISTRY" ]]; then
    echo "MODE_RELAY_NPM_REGISTRY=${NPM_REGISTRY}"
  fi
  if [[ -n "$CLOUD_ENROLL_TOKEN" ]]; then
    echo "MODE_CLOUD_ENROLL_TOKEN=${CLOUD_ENROLL_TOKEN}"
  fi
  if [[ -n "$CLOUD_ACCOUNT_PEPPER" ]]; then
    echo "MODE_CLOUD_ACCOUNT_PEPPER=${CLOUD_ACCOUNT_PEPPER}"
  fi
  if [[ -n "$CLOUD_DATA_DIR" ]]; then
    echo "MODE_CLOUD_DATA_DIR=${CLOUD_DATA_DIR}"
  fi
  if [[ -n "$CLOUD_WECHAT_TOKEN" ]]; then
    echo "MODE_CLOUD_WECHAT_TOKEN=${CLOUD_WECHAT_TOKEN}"
  fi
  if [[ -n "$CLOUD_WECHAT_ALLOWED_USER" ]]; then
    echo "MODE_CLOUD_WECHAT_ALLOWED_USER=${CLOUD_WECHAT_ALLOWED_USER}"
  fi
  if [[ -n "$CLOUD_WECHAT_BASE_URL" ]]; then
    echo "MODE_CLOUD_WECHAT_BASE_URL=${CLOUD_WECHAT_BASE_URL}"
  fi
} > "$INSTALL_DIR/.env"
chmod 600 "$INSTALL_DIR/.env"

cd "$INSTALL_DIR"
# shellcheck disable=SC2086
"${COMPOSE[@]}" up -d $BUILD_FLAG

HEALTH_HOST="127.0.0.1"
if [[ "$BIND" != "0.0.0.0" && "$BIND" != "::" ]]; then
  HEALTH_HOST="$BIND"
fi

healthy=0
for _ in $(seq 1 40); do
  if curl -fsS "http://${HEALTH_HOST}:${PORT}/_health" >/dev/null 2>&1; then
    healthy=1
    break
  fi
  sleep 1
done

echo
if [[ "$healthy" == "1" ]]; then
  echo "中继已启动并通过健康检查：http://${HEALTH_HOST}:${PORT}/_health"
else
  echo "容器已启动，但健康检查未通过。查看日志：${COMPOSE[*]} logs --tail=100 relay" >&2
  exit 1
fi

echo "安装目录：$INSTALL_DIR"
echo "中继令牌：$INSTALL_DIR/.env（查看：cat $INSTALL_DIR/.env）"
echo "对外地址：$PUBLIC_BASE_URL（需由 Caddy/nginx 提供 TLS）"
echo
echo "下一步（被控端隧道）："
echo "  MODE_RELAY_TOKEN='<.env 中的令牌>' \\"
echo "  pnpm --filter @mode/cloud-relay exec tsx src/cli-tunnel.ts \\"
echo "    --relay 'wss://<你的域名>/_tunnel' \\"
echo "    --local 'http://127.0.0.1:45880' \\"
echo "    --generate"
