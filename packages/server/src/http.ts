/* eslint-disable max-lines -- HTTP、WebSocket 与静态资源路由集中注册，保持同一鉴权顺序。 */
import { randomUUID } from "node:crypto";
import { readFile, stat } from "node:fs/promises";
import { basename, extname, relative, resolve, sep } from "node:path";
import { hostname } from "node:os";
import { Hono, type Context } from "hono";
import { serve } from "@hono/node-server";
import { createNodeWebSocket } from "@hono/node-ws";
import type { WebSocket } from "ws";
import {
  Emitter,
  VSBuffer,
  SocketProtocol,
  ChannelServer,
  LoggingChannelServer,
  ProxyChannel,
  type ISocket,
} from "@mode/rpc";
import {
  ServiceCollection,
  IModeAgentService,
  createModeAgentConnectionScope,
  IFileService,
  IGitService,
  ISystemService,
  ITerminalService,
  IBotsService,
  IProviderProvisioningTargetService,
} from "@mode/services";
import {
  botProviders,
  formatLogPrefix,
  formatZodError,
  lanAccessPairRequestSchema,
  remoteTargetSchema,
  type LanAccessPairingEndpoint,
  SERVER_REMOTE_PROTOCOL_VERSION,
  MODE_RPC_HOST_CAPABILITY_HEADER,
  MODE_VERSION,
  type BotProvider,
  type ServerRemoteInfo,
  type ServerRemoteWorkspaceInfo,
} from "@mode/shared";
import { connectRemote, createRemoteBackend, type RemoteConnection } from "./remote/index.js";
import { createHostCapabilityStore } from "./hostCapability.js";

function wrapWebSocket(ws: WebSocket): ISocket {
  const onData = new Emitter<VSBuffer>();
  const onClose = new Emitter<void>();
  const onEnd = new Emitter<void>();

  ws.on("message", (raw: Buffer | ArrayBuffer | Buffer[]) => {
    const buf = Buffer.isBuffer(raw) ? raw : Buffer.from(raw as ArrayBuffer);
    onData.fire(VSBuffer.wrap(new Uint8Array(buf)));
  });
  ws.on("close", () => {
    onClose.fire();
    onEnd.fire();
  });
  ws.on("error", () => {
    onClose.fire();
    onEnd.fire();
  });

  return {
    onData: onData.event,
    onClose: onClose.event,
    onEnd: onEnd.event,
    write(buffer: VSBuffer) {
      if (ws.readyState === ws.OPEN) {
        ws.send(buffer.buffer);
      }
    },
    end() {
      ws.close();
    },
    drain() {
      return Promise.resolve();
    },
    dispose() {
      ws.close();
    },
  };
}

const log = (...args: unknown[]) =>
  console.log(formatLogPrefix("mode-server:http", process.pid), ...args);

function setupChannelServer(
  ws: WebSocket,
  services: ServiceCollection,
  clientMode: "desktop-continuous" | "web-remote-replayable",
) {
  const socket = wrapWebSocket(ws);
  const protocol = new SocketProtocol(socket);
  const rawServer = new ChannelServer(protocol, "server");
  // 用日志中间件包装，统一记录所有 RPC 调用
  const server = new LoggingChannelServer(rawServer, log);
  const agentService = services.getOptional(IModeAgentService);
  const connectionScope = agentService
    ? createModeAgentConnectionScope(agentService, {
        connectionId: `server-ws-${randomUUID()}`,
        clientMode,
        role: clientMode === "desktop-continuous" ? "trusted-host-relay" : "terminal-client",
      })
    : undefined;
  const overrides = new Map<string, unknown>();
  if (connectionScope) {
    overrides.set(IModeAgentService.channelName, connectionScope.service);
  }
  // Provisioning 携带跨 Environment 凭据，只允许 Desktop trusted host 使用；普通 Web
  // remote/replayable 客户端即使知道频道名，也不能获得 target 写入接口。
  // Provider Provisioning 的「写入凭据」接口永远不通过 HTTP/WS 服务暴露：
  // 非受信客户端一律拒绝；受信客户端（局域网对端）拿到的是一个会立即失败的实现——
  // 一是对端环境本就不该被写入我们的凭据，二是缺省会让调用方的 initialSync 永久挂起
  // （ChannelClient 对未注册频道会排队等待，局域网连接实测踩到过）。
  if (services.getOptional(IProviderProvisioningTargetService)) {
    overrides.set(IProviderProvisioningTargetService.channelName, {
      apply: async () => {
        throw new Error(
          clientMode === "desktop-continuous"
            ? "Provider Provisioning 目标不可用：该环境不支持写入凭据"
            : "Provider Provisioning 仅支持受信 Desktop Host",
        );
      },
    });
  } else {
    // 本地 host 没有注册 provisioning target 服务，overrides 只能替换已注册服务、无法新增频道；
    // 这里显式注册一个占位通道：否则客户端会一直排队等这个频道（ChannelClient 对未注册频道不报错）。
    server.registerChannel(
      IProviderProvisioningTargetService.channelName,
      ProxyChannel.fromService({
        apply: async () => {
          throw new Error("Provider Provisioning 目标不可用：该环境不支持写入凭据");
        },
      }),
    );
  }
  services.exposeOnChannelServer(server, overrides);
  socket.onClose(() => {
    void connectionScope?.dispose();
    rawServer.dispose();
  });
}

/** 存储 web 模式下的远程连接，key 为随机 ID */
const remoteConnections = new Map<string, RemoteConnection>();

function generateId(): string {
  return Math.random().toString(36).slice(2) + Date.now().toString(36);
}

interface HttpServerOptions {
  serverId?: string;
  name?: string;
  host?: string;
  authRequired?: boolean;
  authToken?: string;
  /**
   * 多客户端令牌校验：局域网访问下每个已配对设备各有令牌，服务端逐令牌校验；
   * 提供时优先于单一 authToken 比较。
   */
  verifyAuthToken?: (token: string) => boolean;
  /** 局域网配对端点：提供时暴露 POST /api/lan/pair（该路由用一次性配对码自证，不要求既有令牌）。 */
  lanPairing?: LanAccessPairingEndpoint;
  spaFallback?: boolean;
  staticRoot?: string;
  /** 工作区列表：静态数组或按请求求值（局域网服务端要反映「刚刚打开的工作区」）。 */
  workspaces?: ServerRemoteWorkspaceInfo[] | (() => Promise<ServerRemoteWorkspaceInfo[]>);
}

function readTrimmedEnv(name: string): string | undefined {
  const value = process.env[name]?.trim();
  return value ? value : undefined;
}

function resolveServerId(options: HttpServerOptions): string {
  return (
    options.serverId?.trim() || readTrimmedEnv("MODE_SERVER_ID") || hostname() || "mode-server"
  );
}

async function resolveServerWorkspaces(options: HttpServerOptions): Promise<ServerRemoteWorkspaceInfo[]> {
  if (typeof options.workspaces === "function") {
    try {
      return await options.workspaces();
    } catch {
      return [];
    }
  }
  if (options.workspaces) {
    return options.workspaces;
  }
  const workspacePath = readTrimmedEnv("MODE_SERVER_WORKSPACE") || process.cwd();
  return [
    {
      path: workspacePath,
      label: basename(workspacePath) || workspacePath,
    },
  ];
}

async function createServerInfo(options: HttpServerOptions): Promise<ServerRemoteInfo> {
  return {
    serverId: resolveServerId(options),
    ...(options.name?.trim() || readTrimmedEnv("MODE_SERVER_NAME")
      ? { name: options.name?.trim() || readTrimmedEnv("MODE_SERVER_NAME") }
      : {}),
    version: MODE_VERSION,
    protocolVersion: SERVER_REMOTE_PROTOCOL_VERSION,
    authRequired: options.authRequired ?? Boolean(readTrimmedEnv("MODE_SERVER_TOKEN")),
    workspaces: await resolveServerWorkspaces(options),
    capabilities: {
      desktopContinuous: true,
      websocketRpc: true,
    },
  };
}

const modeLiteTokenCookieName = "mode_lite_token";
// 改名（ZCODIUM → Mode）前的 cookie 名：老客户端/手机浏览器里可能还留着它，
// 读时兜底一次，避免升级后每次都要重新走 ?token= 链接。
const legacyLiteTokenCookieName = "zcode_lite_token";

const staticMimeTypes: Record<string, string> = {
  ".css": "text/css; charset=utf-8",
  ".gif": "image/gif",
  ".html": "text/html; charset=utf-8",
  ".ico": "image/x-icon",
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".js": "text/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".map": "application/json; charset=utf-8",
  ".png": "image/png",
  ".svg": "image/svg+xml",
  ".txt": "text/plain; charset=utf-8",
  ".wasm": "application/wasm",
  ".webp": "image/webp",
  ".woff": "font/woff",
  ".woff2": "font/woff2",
};

function parseCookieHeader(header: string | undefined): Map<string, string> {
  const cookies = new Map<string, string>();
  if (!header) {
    return cookies;
  }
  for (const part of header.split(";")) {
    const separator = part.indexOf("=");
    if (separator <= 0) {
      continue;
    }
    const name = part.slice(0, separator).trim();
    const value = part.slice(separator + 1).trim();
    if (name) {
      cookies.set(name, value);
    }
  }
  return cookies;
}

function readPresentedLiteToken(c: Context): { token: string | undefined; fromQuery: boolean } {
  const url = new URL(c.req.url);
  const queryToken = url.searchParams.get("token");
  if (queryToken) {
    return { token: queryToken, fromQuery: true };
  }
  const cookies = parseCookieHeader(c.req.header("cookie"));
  return {
    token: cookies.get(modeLiteTokenCookieName) ?? cookies.get(legacyLiteTokenCookieName),
    fromQuery: false,
  };
}

/** 校验通过且令牌来自 query 时落一次 Cookie；局域网多客户端场景复用同一入口。 */
function rememberLiteTokenCookie(c: Context, token: string): void {
  c.header(
    "Set-Cookie",
    `${modeLiteTokenCookieName}=${encodeURIComponent(token)}; Path=/; HttpOnly; SameSite=Lax`,
  );
}

function isTokenProtectedPath(pathname: string): boolean {
  return pathname === "/ws" || pathname.startsWith("/ws/") || pathname.startsWith("/api/");
}

function isStaticFallbackAllowed(pathname: string): boolean {
  return !isTokenProtectedPath(pathname);
}

function isInsideDirectory(root: string, candidate: string): boolean {
  const diff = relative(root, candidate);
  return diff === "" || (!diff.startsWith("..") && !diff.includes(`..${sep}`));
}

async function resolveStaticFile(
  staticRoot: string,
  pathname: string,
  spaFallback: boolean,
): Promise<string | null> {
  const root = resolve(staticRoot);
  const normalizedPathname = pathname === "/" ? "/index.html" : pathname;
  const relativePath = decodeURIComponent(normalizedPathname).replace(/^\/+/, "");
  let candidate = resolve(root, relativePath);
  if (!isInsideDirectory(root, candidate)) {
    return null;
  }

  try {
    const candidateStat = await stat(candidate);
    if (candidateStat.isDirectory()) {
      candidate = resolve(candidate, "index.html");
      if (!isInsideDirectory(root, candidate)) {
        return null;
      }
      const indexStat = await stat(candidate);
      return indexStat.isFile() ? candidate : null;
    }
    if (candidateStat.isFile()) {
      return candidate;
    }
  } catch {
    // 静态资源未命中时再进入 SPA fallback，保留真实文件错误的 404 语义。
  }

  if (!spaFallback || !isStaticFallbackAllowed(pathname)) {
    return null;
  }
  const indexFile = resolve(root, "index.html");
  try {
    const indexStat = await stat(indexFile);
    return indexStat.isFile() ? indexFile : null;
  } catch {
    return null;
  }
}

function staticContentType(filePath: string): string {
  return staticMimeTypes[extname(filePath).toLowerCase()] ?? "application/octet-stream";
}

export function createHttpServer(
  services: ServiceCollection,
  port = 3030,
  options: HttpServerOptions = {},
) {
  const app = new Hono();
  const { injectWebSocket, upgradeWebSocket } = createNodeWebSocket({ app });
  const hostCapabilities = createHostCapabilityStore();

  const authToken = options.authToken?.trim();
  const verifyAuthToken = options.verifyAuthToken;
  if (authToken || verifyAuthToken) {
    app.use("*", async (c, next) => {
      const pathname = new URL(c.req.url).pathname;
      // 配对接口是「登录前引导」：客户端此时没有令牌，凭一次性配对码换取。
      if (pathname === "/api/lan/pair") {
        await next();
        return;
      }
      const presented = readPresentedLiteToken(c);
      const validToken =
        presented.token !== undefined &&
        (verifyAuthToken ? verifyAuthToken(presented.token) : presented.token === authToken);
      if (validToken && presented.fromQuery && presented.token) {
        rememberLiteTokenCookie(c, presented.token);
      }
      if (!isTokenProtectedPath(pathname) || validToken) {
        await next();
        return;
      }
      return c.json({ error: "Unauthorized" }, 401);
    });
  }

  app.get("/api/server-info", async (c) => c.json(await createServerInfo(options)));
  app.post("/api/rpc-host-capability", (c) => c.json(hostCapabilities.issue()));

  // 局域网配对：一次性配对码 → 该设备专属长期令牌。失败不区分「码错/过期」，
  // 避免给扫描者提供探测信号。
  if (options.lanPairing) {
    const lanPairing = options.lanPairing;
    app.post("/api/lan/pair", async (c) => {
      let rawBody: unknown;
      try {
        rawBody = await c.req.json();
      } catch {
        return c.json({ error: "Invalid JSON body" }, 400);
      }
      const parsedBody = lanAccessPairRequestSchema.safeParse(rawBody);
      if (!parsedBody.success) {
        return c.json({ error: `Invalid request body: ${formatZodError(parsedBody.error)}` }, 400);
      }
      const result = await lanPairing.redeem(parsedBody.data);
      if (!result) {
        return c.json({ error: "Invalid or expired pair code" }, 401);
      }
      return c.json(result);
    });
  }

  // 普通 `/ws` 永远是 terminal-client；浏览器/任意客户端设置旧 mode header
  // 都不能再把自己提升为 trusted host。
  app.get(
    "/ws",
    upgradeWebSocket(() => ({
      onOpen(_event, ws) {
        setupChannelServer(ws.raw as WebSocket, services, "web-remote-replayable");
      },
    })),
  );

  const upgradeTrustedHostWebSocket = upgradeWebSocket(() => ({
    onOpen(_event, ws) {
      setupChannelServer(ws.raw as WebSocket, services, "desktop-continuous");
    },
  }));
  app.use("/ws/host", async (c, next) => {
    const capability = c.req.header(MODE_RPC_HOST_CAPABILITY_HEADER);
    if (!hostCapabilities.consume(capability)) {
      return c.json({ error: "Invalid or expired host capability" }, 401);
    }
    await next();
  });
  app.get("/ws/host", upgradeTrustedHostWebSocket);

  // Web 模式下发起远程连接
  app.post("/api/connect-remote", async (c) => {
    const rawBody = await c.req.json();
    const parsedBody = remoteTargetSchema.safeParse(rawBody);
    if (!parsedBody.success) {
      return c.json({ error: `Invalid request body: ${formatZodError(parsedBody.error)}` }, 400);
    }
    const body = parsedBody.data;

    try {
      const backend = await createRemoteBackend(body);
      const connection = await connectRemote(backend);
      const id = generateId();
      remoteConnections.set(id, connection);

      return c.json({ id });
    } catch (err: unknown) {
      const message = err instanceof Error ? err.message : String(err);
      return c.json({ error: message }, 500);
    }
  });

  const handleBotCallback = async (c: Context) => {
    const provider = c.req.param("provider") as BotProvider;
    if (!botProviders.includes(provider)) {
      return c.json({ error: `Unsupported provider: ${provider}` }, 400);
    }
    if (provider !== "webhook") {
      return c.json({ error: `Provider ${provider} does not support HTTP callbacks.` }, 400);
    }
    const botsService = services.getOptional(IBotsService);
    if (!botsService) {
      return c.json({ error: "Bots service is not available." }, 503);
    }
    const rawBodyText = await c.req.text().catch(() => "");
    let rawBody: unknown = {};
    if (rawBodyText) {
      try {
        rawBody = JSON.parse(rawBodyText) as unknown;
      } catch {
        rawBody = { payload: rawBodyText };
      }
    }
    const webhookSecret = c.req.header("x-mode-bot-secret");
    const botId = c.req.param("botId");
    const result = await botsService.handleProviderCallbackResponse(provider, {
      ...(typeof rawBody === "object" && rawBody !== null ? rawBody : { payload: rawBody }),
      rawBody: rawBodyText,
      ...(botId ? { botId } : {}),
      ...(webhookSecret ? { webhookSecret } : {}),
    });
    const responseBody = result.responseBody ?? { ok: result.ok, replies: result.replies };
    if (result.status === 400) {
      return c.json(responseBody, 400);
    }
    if (result.status === 401) {
      return c.json(responseBody, 401);
    }
    if (result.status === 503) {
      // Bugfix：Bot 业务失败必须把可重试状态透传给 HTTP provider；返回 200 会让
      // webhook/网关误以为消息已消费，效果与提前提交 Telegram offset 相同。
      return c.json(responseBody, 503);
    }
    return c.json(responseBody, 200);
  };

  app.post("/api/bots/:provider", handleBotCallback);
  app.post("/api/bots/:provider/:botId", handleBotCallback);

  // 远程连接的 WebSocket 端点，将远程 services 桥接给浏览器
  app.get(
    "/ws/remote/:id",
    upgradeWebSocket((c) => {
      const id = c.req.param("id");
      return {
        onOpen(_event, ws) {
          if (!id) {
            ws.close(4000, "Missing remote connection id");
            return;
          }
          const connection = remoteConnections.get(id);
          if (!connection) {
            ws.close(4004, "Remote connection not found");
            return;
          }
          // 一个连接只给一个 WS 客户端使用，取出后从 Map 移除
          remoteConnections.delete(id);

          // 将远程 services 包装为 ServiceCollection，复用 exposeOnChannelServer 统一注册
          const remoteServices = new ServiceCollection()
            .register(IFileService, connection.services.fileService)
            .register(IGitService, connection.services.gitService)
            .register(ISystemService, connection.services.systemService)
            .register(ITerminalService, connection.services.terminalService);

          setupChannelServer(ws.raw as WebSocket, remoteServices, "web-remote-replayable");
        },
      };
    }),
  );

  if (options.staticRoot?.trim()) {
    const staticRoot = options.staticRoot.trim();
    app.get("*", async (c) => {
      const pathname = new URL(c.req.url).pathname;
      const filePath = await resolveStaticFile(staticRoot, pathname, options.spaFallback ?? true);
      if (!filePath) {
        return c.notFound();
      }
      return c.body(await readFile(filePath), 200, {
        "Cache-Control": filePath.endsWith("index.html")
          ? "no-cache"
          : "public, max-age=31536000, immutable",
        "Content-Type": staticContentType(filePath),
      });
    });
  }

  const server = serve({ fetch: app.fetch, hostname: options.host, port }, () => {
    const address = server.address();
    const listenPort = typeof address === "object" && address ? address.port : port;
    const listenHost = options.host?.trim() || "localhost";
    log(`http://${listenHost}:${listenPort}`);
  });

  injectWebSocket(server);

  return server;
}
