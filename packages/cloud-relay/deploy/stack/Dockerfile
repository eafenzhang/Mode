# Mode 单容器部署镜像：cloud-relay（8787）+ Mode Go relay（8788）+ 官网（80）。
# 构建上下文需包含 cloud-relay/（package.json + src）与同目录的 start.sh。
ARG NODE_IMAGE=node:24-alpine
FROM ${NODE_IMAGE}

ARG NPM_REGISTRY=https://registry.npmjs.org

WORKDIR /app
COPY cloud-relay/package.json ./
# tsx 在 devDependencies；显式包含 dev 依赖，避免运行期 npx 联网下载。
RUN npm install --include=dev --no-audit --no-fund --registry="${NPM_REGISTRY}"
COPY cloud-relay/src ./src
COPY start.sh /app/start.sh
RUN chmod 755 /app/start.sh

ENV MODE_RELAY_HOST=0.0.0.0 \
    MODE_RELAY_PORT=8787 \
    SITE_PORT=80 \
    SITE_ROOT=/srv/public

EXPOSE 8787 8788 80

CMD ["/app/start.sh"]
