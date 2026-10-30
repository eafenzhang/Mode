#!/bin/sh
# 单容器多进程入口：Mode Cloud / Mode Go relay / 官网静态站。
# 任一进程退出即整体退出，交给 compose restart 策略统一重启，
# 避免「容器在跑但某个服务已死」的半死状态。
set -eu

pids=""
cleanup() {
  for pid in $pids; do
    kill "$pid" 2>/dev/null || true
  done
}
trap cleanup TERM INT

# 1) Mode Cloud：控制面 /v1/*、WSS 中继 /_tunnel、网页门户 /
( cd /app && exec ./node_modules/.bin/tsx src/cli-server.ts ) & pids="$pids $!"

# 2) Mode Go relay：源码由宿主机挂载到 /app/agent-go（含 node_modules）
( cd /app/agent-go && exec node src/server.mjs ) & pids="$pids $!"

# 3) 官网静态站：server.js 由宿主机挂载到 /srv，站点文件在 /srv/public
( cd /srv && exec node server.js ) & pids="$pids $!"

# busybox ash 支持 wait -n；不支持时退回等待全部子进程。
wait -n 2>/dev/null || wait
cleanup
