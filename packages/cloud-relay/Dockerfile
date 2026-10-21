ARG NODE_IMAGE=node:24-alpine
FROM ${NODE_IMAGE}

ARG NPM_REGISTRY=https://registry.npmjs.org

WORKDIR /app
COPY package.json ./
RUN npm install --no-audit --no-fund --registry="${NPM_REGISTRY}"
COPY src ./src

ENV MODE_RELAY_HOST=0.0.0.0
ENV MODE_RELAY_PORT=8787
EXPOSE 8787

CMD ["npx", "tsx", "src/cli-server.ts"]
