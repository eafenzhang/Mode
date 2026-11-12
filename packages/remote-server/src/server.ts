import { timingSafeEqual } from "node:crypto";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { createServer as createHttpsServer } from "node:https";

import { createApiHandler } from "./api.js";
import { RemoteGateway } from "./gateway.js";
import { createGithubVerifier, type GitHubVerifier } from "./github.js";
import { IceServerProvider } from "./webrtc.js";
import { RemoteMetrics } from "./metrics.js";
import { CloudStore } from "./store.js";

type Server = ReturnType<typeof createServer> | ReturnType<typeof createHttpsServer>;

export interface RemoteServerOptions {
  port?: number;
  host?: string;
  secret: string;
  dbPath: string;
  pepper?: string;
  publicUrl?: string;
  nodeId?: string;
  allowedOrigins?: string[];
  githubVerifier?: GitHubVerifier;
  /** Browser GitHub Device Flow 代理所需 OAuth App client id。 */
  githubClientId?: string;
  /** GitHub OAuth App client secret；只保存在服务端，用于回调换 token。 */
  githubClientSecret?: string;
  /** GitHub OAuth App 回调地址；未提供时按 publicUrl 派生。 */
  githubOAuthCallbackUrl?: string;
  /** 授权页基址（测试/自建代理用）；生产缺省 https://github.com。 */
  githubOAuthAuthorizeBaseUrl?: string;
  /** token 交换基址；缺省复用 githubDeviceBaseUrl 或 https://github.com。 */
  githubOAuthTokenBaseUrl?: string;
  /** 测试/自建 GitHub 代理基址；生产缺省 https://github.com。 */
  githubDeviceBaseUrl?: string;
  /** 直接终止 TLS（生产也可由反向代理终止）；提供时 URL/Relay 会使用 https/wss。 */
  tls?: { key: string | Buffer; cert: string | Buffer };
  /** WebRTC/P2P 选路的 ICE 配置；未配置时 connect.* 不带 iceServers。 */
  webrtc?: import("./gatewayTypes.js").WebRtcConfig;
  /** 测试注入：Cloudflare TURN 凭证请求使用的 fetch。 */
  fetchImpl?: typeof fetch;
  /** 提供时暴露 `GET /metrics`（Bearer / x-metrics-token）；未配置时返回 404。 */
  metricsToken?: string;
  log?: (line: string) => void;
}

export interface RemoteServerHandle {
  readonly port: number;
  readonly url: string;
  readonly relayEndpoint: string;
  readonly store: CloudStore;
  readonly gateway: RemoteGateway;
  close(): Promise<void>;
}

export async function startRemoteServer(options: RemoteServerOptions): Promise<RemoteServerHandle> {
  if (options.secret.trim().length < 16) {
    throw new Error("remote server secret 至少 16 字符");
  }
  const log = options.log ?? ((line: string) => console.log(`[remote-server] ${line}`));
  const store = new CloudStore(options.dbPath);
  const metrics = new RemoteMetrics();
  const secure = Boolean(options.tls);
  let relayEndpoint = options.publicUrl
    ? toWebSocketEndpoint(options.publicUrl)
    : `${secure ? "wss" : "ws"}://127.0.0.1:${options.port ?? 0}/ws/v1/relay`;
  const iceServers = new IceServerProvider(options.webrtc, {
    log,
    ...(options.fetchImpl ? { fetchImpl: options.fetchImpl } : {}),
  });
  const gateway = new RemoteGateway({
    store,
    secret: options.secret,
    relayEndpoint: () => relayEndpoint,
    nodeId: options.nodeId,
    allowedOrigins: options.allowedOrigins,
    ...(options.webrtc ? { webrtc: options.webrtc } : {}),
    iceServers: (deviceId) => iceServers.get(deviceId),
    metrics,
    log,
  });
  const githubClientId = options.githubClientId?.trim() ?? "";
  const githubClientSecret = options.githubClientSecret?.trim() ?? "";
  const oauthCallbackUrl =
    options.githubOAuthCallbackUrl?.trim() ||
    (options.publicUrl
      ? `${options.publicUrl.replace(/\/+$/u, "")}/api/v1/auth/github/oauth/callback`
      : "");
  const oauthTokenBaseUrl =
    options.githubOAuthTokenBaseUrl?.trim() || options.githubDeviceBaseUrl?.trim();
  const api = createApiHandler({
    store,
    gateway,
    pepper: options.pepper ?? options.secret,
    github: options.githubVerifier ?? createGithubVerifier(),
    ...(options.allowedOrigins?.length ? { allowedOrigins: options.allowedOrigins } : {}),
    ...(options.publicUrl ? { publicUrl: options.publicUrl } : {}),
    ...(options.webrtc ? { webrtc: options.webrtc } : {}),
    ...(githubClientId
      ? {
          githubDevice: {
            clientId: githubClientId,
            ...(options.githubDeviceBaseUrl ? { baseUrl: options.githubDeviceBaseUrl } : {}),
          },
        }
      : {}),
    ...(githubClientId && githubClientSecret && oauthCallbackUrl
      ? {
          githubOAuth: {
            clientId: githubClientId,
            clientSecret: githubClientSecret,
            callbackUrl: oauthCallbackUrl,
            ...(options.githubOAuthAuthorizeBaseUrl
              ? { authorizeBaseUrl: options.githubOAuthAuthorizeBaseUrl }
              : {}),
            ...(oauthTokenBaseUrl ? { tokenBaseUrl: oauthTokenBaseUrl } : {}),
            log,
          },
        }
      : {}),
  });
  log(
    `github_oauth=${githubClientId && githubClientSecret && oauthCallbackUrl ? "enabled" : "disabled"}`,
  );

  const metricsToken = options.metricsToken?.trim() ?? "";
  const handler = (req: IncomingMessage, res: ServerResponse): void => {
    void handleRequest(api, req, res, {
      metrics,
      ready: () => store.ping(),
      ...(metricsToken ? { metricsToken } : {}),
    });
  };
  const server = options.tls ? createHttpsServer(options.tls, handler) : createServer(handler);
  server.on("upgrade", (req, socket, head) => {
    const url = new URL(req.url ?? "/", "http://remote.local");
    if (!gateway.handleUpgrade(req, socket, head, url)) {
      socket.destroy();
    }
  });

  // Cloudflare 凭证必须在外接连接前准备好；失败只记日志，不阻塞启动（无 TURN 时回落 Relay）。
  await iceServers.warmup();
  const turnRefreshTimer = setInterval(() => iceServers.maybeRefresh(), 5 * 60_000);
  turnRefreshTimer.unref?.();

  await listen(server, options.port ?? 0, options.host ?? "127.0.0.1");
  const address = server.address();
  if (!address || typeof address === "string") {
    await closeServer(server);
    store.close();
    throw new Error("无法获取 remote server 监听地址");
  }
  const port = address.port;
  const url = `${secure ? "https" : "http"}://${options.host ?? "127.0.0.1"}:${port}`;
  if (!options.publicUrl) {
    relayEndpoint = `${secure ? "wss" : "ws"}://127.0.0.1:${port}/ws/v1/relay`;
  }
  log(`listening on ${url}`);

  return {
    port,
    url,
    relayEndpoint,
    store,
    gateway,
    async close() {
      clearInterval(turnRefreshTimer);
      gateway.close();
      await closeServer(server);
      store.close();
    },
  };
}

async function handleRequest(
  api: (req: IncomingMessage, res: ServerResponse, url: URL) => Promise<void>,
  req: IncomingMessage,
  res: ServerResponse,
  metricsOptions: {
    metrics: RemoteMetrics;
    ready: () => boolean;
    metricsToken?: string;
  },
): Promise<void> {
  const url = new URL(req.url ?? "/", "http://remote.local");
  if (url.pathname === "/health") {
    res.writeHead(200, { "content-type": "text/plain" });
    res.end("ok");
    return;
  }
  if (url.pathname === "/ready") {
    const ready = metricsOptions.ready();
    res.writeHead(ready ? 200 : 503, { "content-type": "text/plain" });
    res.end(ready ? "ready" : "not-ready");
    return;
  }
  if (url.pathname === "/metrics") {
    const expected = metricsOptions.metricsToken;
    if (!expected) {
      res.writeHead(404, { "content-type": "application/json; charset=utf-8" });
      res.end(JSON.stringify({ error: { code: "not_found", message: "未知路径" } }));
      return;
    }
    const authorization = req.headers.authorization;
    const presented =
      typeof authorization === "string" && authorization.startsWith("Bearer ")
        ? authorization.slice("Bearer ".length)
        : typeof req.headers["x-metrics-token"] === "string"
          ? req.headers["x-metrics-token"]
          : "";
    if (!constantTimeEqual(presented, expected)) {
      res.writeHead(401, { "content-type": "application/json; charset=utf-8" });
      res.end(JSON.stringify({ error: { code: "unauthorized", message: "metrics token 无效" } }));
      return;
    }
    res.writeHead(200, { "content-type": "text/plain; version=0.0.4; charset=utf-8" });
    res.end(metricsOptions.metrics.render());
    return;
  }
  if (url.pathname.startsWith("/api/v1/")) {
    metricsOptions.metrics.apiRequested();
    await api(req, res, url);
    return;
  }
  res.writeHead(404, { "content-type": "application/json; charset=utf-8" });
  res.end(JSON.stringify({ error: { code: "not_found", message: "未知路径" } }));
}

function constantTimeEqual(left: string, right: string): boolean {
  const a = Buffer.from(left, "utf8");
  const b = Buffer.from(right, "utf8");
  return a.length === b.length && a.length > 0 && timingSafeEqual(a, b);
}

function toWebSocketEndpoint(publicUrl: string): string {
  // 子路径形态（fnOS 网关 / 反代）：返回相对 relay 路径，由客户端按服务地址解析。
  if (publicUrl.startsWith("/")) {
    return `${publicUrl.replace(/\/+$/u, "")}/ws/v1/relay`;
  }
  const url = new URL(publicUrl);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  // 保留 publicUrl 的路径前缀（fnOS 网关/反向代理子路径），只追加 Relay 端点。
  url.pathname = `${url.pathname.replace(/\/+$/u, "")}/ws/v1/relay`;
  url.search = "";
  url.hash = "";
  return url.toString();
}

function listen(server: Server, port: number, host: string): Promise<void> {
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(port, host, () => {
      server.off("error", reject);
      resolve();
    });
  });
}

function closeServer(server: Server): Promise<void> {
  return new Promise((resolve) => {
    server.close(() => resolve());
    server.closeAllConnections?.();
  });
}
