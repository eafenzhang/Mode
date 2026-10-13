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
  type IServerChannel,
  type IChannelServer,
  type CancellationToken,
  type Event,
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
  lanAccessEnableRequestSchema,
  lanAccessPairRequestSchema,
  remoteTargetSchema,
  type LanAccessPairingEndpoint,
  type LanAccessState,
  SERVER_REMOTE_PROTOCOL_VERSION,
  MODE_RPC_HOST_CAPABILITY_HEADER,
  MODE_VERSION,
  type BotProvider,
  type ServerRemoteInfo,
  type ServerRemoteWorkspaceInfo,
} from "@mode/shared";
// 远程桌面协议面单独入口（T1），根入口不重导出，避免把桌面链路类型带进所有消费方。
import {
  remoteDesktopQualitySchema,
  type RemoteDesktopErrorCode,
  type RemoteDesktopQuality,
} from "@mode/shared/remote-desktop";
import { connectRemote, createRemoteBackend, type RemoteConnection } from "./remote/index.js";
import { createHostCapabilityStore } from "./hostCapability.js";
import { createRemoteDesktopTicketStore } from "./remoteDesktopTicket.js";

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

/** 只有顶层对象上的 workspacePath 才算「打开工作区」信号；readdir/resolvePath 用 path，不录。 */
function extractWorkspacePathArg(arg: unknown): string | null {
  if (!arg || typeof arg !== "object") {
    return null;
  }
  const path = (arg as { workspacePath?: unknown }).workspacePath;
  return typeof path === "string" && path.trim() ? path : null;
}

class LanRecordingServerChannel<TContext> implements IServerChannel<TContext> {
  constructor(
    private inner: IServerChannel<TContext>,
    private notify: (workspacePath: string) => void,
  ) {}

  call<T>(
    ctx: TContext,
    command: string,
    arg?: unknown,
    cancellationToken?: CancellationToken,
  ): Promise<T> {
    const workspacePath = extractWorkspacePathArg(arg);
    if (workspacePath) {
      this.notify(workspacePath);
    }
    return this.inner.call(ctx, command, arg, cancellationToken);
  }

  listen<T>(ctx: TContext, event: string, arg?: unknown): Event<T> {
    const workspacePath = extractWorkspacePathArg(arg);
    if (workspacePath) {
      this.notify(workspacePath);
    }
    return this.inner.listen(ctx, event, arg);
  }
}

/**
 * 局域网「最近打开目录」录制（spec: docs/specs/lan-paired-devices.md）：
 * 包装 registerChannel，对该连接上每个频道的 call/listen 入参检查 workspacePath。
 * 录制在调用入口进行——只要对端发起了携带 workspacePath 的调用就视为「打开」，
 * 调用成败不影响记录；纯目录浏览（path 字段）不会命中。
 */
class LanWorkspaceRecordingChannelServer<TContext> implements IChannelServer<TContext> {
  constructor(
    private inner: IChannelServer<TContext>,
    private notify: (workspacePath: string) => void,
  ) {}

  registerChannel(channelName: string, channel: IServerChannel<TContext>): void {
    this.inner.registerChannel(
      channelName,
      new LanRecordingServerChannel(channel, this.notify),
    );
  }

  ready(): void {
    this.inner.ready?.();
  }
}

function setupChannelServer(
  ws: WebSocket,
  services: ServiceCollection,
  clientMode: "desktop-continuous" | "web-remote-replayable",
  lanRecording?: { clientId: string; onWorkspace: (clientId: string, workspacePath: string) => void },
) {
  const socket = wrapWebSocket(ws);
  const protocol = new SocketProtocol(socket);
  const rawServer = new ChannelServer(protocol, "server");
  // 用日志中间件包装，统一记录所有 RPC 调用
  const loggedServer = new LoggingChannelServer(rawServer, log);
  const server = lanRecording
    ? new LanWorkspaceRecordingChannelServer(loggedServer, (workspacePath) =>
        lanRecording.onWorkspace(lanRecording.clientId, workspacePath),
      )
    : loggedServer;
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
  /**
   * 局域网多客户端：把 /ws/host 升级连接的长期令牌解析为客户端记录 id。
   * 与鉴权走同一令牌校验路径；未提供或解析不到时该连接不参与录制。
   */
  resolveLanClientByToken?: (token: string) => string | null;
  /** 局域网多客户端：该客户端连接上出现携带 workspacePath 的 RPC 时回调（最近在本机打开的目录）。 */
  onLanClientWorkspace?: (clientId: string, workspacePath: string) => void;
  /**
   * 服务端形态的局域网被连端（docs/specs/lan-host-server-mode.md）：提供时暴露
   * /api/lan/state 与四个动作端点（开关 / 配对码 / 移除对端 / 重置，均走 /api/* 既有令牌中间件）。
   * 配对端点与逐设备令牌校验分别经 lanPairing / verifyAuthToken 组合提供。
   */
  lanAccessHost?: LanAccessHostEndpoint;
  spaFallback?: boolean;
  staticRoot?: string;
  /** 工作区列表：静态数组或按请求求值（局域网服务端要反映「刚刚打开的工作区」）。 */
  workspaces?: ServerRemoteWorkspaceInfo[] | (() => Promise<ServerRemoteWorkspaceInfo[]>);
  /**
   * 设备级电脑控制能力（LAN 能力协商）。只有具备 helper/host 条件的 server
   * （desktop LAN access server）会传布尔值；不传 = 不声明该键（docs/specs/lan-remote-computer-use.md）。
   */
  computerUseCapability?: boolean;
  /**
   * 设备级远程桌面能力位（LAN 能力协商）。只有 win32 被控且总开关开启的 desktop
   * LAN access server 会传值；不传 = 不声明该键（docs/specs/lan-remote-desktop.md 产品规则 5）。
   * 被控总开关是可变设置：函数形式按每次握手现取（同 workspaces 先例），
   * 冻结布尔会让「开关打开」必须重启监听才生效。
   */
  remoteDesktopCapability?: boolean | (() => boolean);
  /**
   * 设备级原生媒体面能力位（纯直替版本门，LAN 能力协商）。与 remoteDesktopCapability
   * 同口径：只有被控 desktop LAN access server 会传值，不传 = 不声明该键；
   * 函数形式按每次握手现取（docs/specs/lan-remote-desktop-native-media.md 版本门 1）。
   */
  remoteDesktopNativeCapability?: boolean | (() => boolean);
  /**
   * 局域网远程桌面被控 hook（docs/specs/lan-remote-desktop.md 接口变更 2）：
   * 提供时暴露 POST /api/remote-desktop/session 与 GET /ws/remote-desktop。
   * 会话请求由宿主裁决（同意流），票据签发与单次消费由本模块兜底；
   * 信令单帧（RemoteDesktopSignalMessage JSON）交给 onSessionSocket 的宿主转发，本模块不解析内容。
   * onMediaSocket 可选：媒体 WS（/ws/remote-desktop-media）的交接口，缺省时该端点 501。
   */
  remoteDesktop?: RemoteDesktopHooks;
}

/** 远程桌面被控 hook：请求会话（可含被控同意）与升级后的信令 / 媒体 socket 交接。 */
export interface RemoteDesktopHooks {
  requestSession(
    clientId: string,
    quality?: RemoteDesktopQuality,
  ): Promise<{ ok: true } | { error: RemoteDesktopErrorCode }>;
  onSessionSocket(clientId: string, socket: WebSocket): void;
  /** 媒体 WS 升级成功后的交接（收发逻辑由宿主侧 relay 承担，本模块不解析帧）。 */
  onMediaSocket?(clientId: string, socket: WebSocket): void;
}

/** 服务端形态局域网被连端的端点面（唯一所有者 = packages/server 的 lanAccessHost）。 */
export interface LanAccessHostEndpoint {
  getState(): LanAccessState;
  setEnabled(enabled: boolean): Promise<LanAccessState>;
  createPairCode(): LanAccessState;
  removeClient(clientId: string): Promise<LanAccessState>;
  resetTokens(): Promise<LanAccessState>;
}

/**
 * 会话请求失败时的 HTTP 状态：稳定错误码是主要契约（对端按 code 出文案），
 * 状态码只做传输层归类（docs/specs/lan-remote-desktop.md 降级与失败语义）。
 */
function remoteDesktopErrorStatus(code: RemoteDesktopErrorCode): 401 | 403 | 409 | 500 | 501 {
  switch (code) {
    case "unauthorized":
    case "stale_ticket":
      return 401;
    case "disabled":
    case "consent_denied":
    case "consent_timeout":
      return 403;
    case "busy":
    // locally_controlled 是主控侧本地闸门（spec 产品规则 9），被控 server 不会产生；
    // 此处仅为枚举穷尽映射，语义与 busy 同类（冲突）。
    case "locally_controlled":
      return 409;
    case "unsupported":
      return 501;
    case "internal":
      return 500;
  }
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
      // computerUse 只在显式声明时发布：不声明 = 不发键，旧对端与其余消费方行为不变
      // （docs/specs/lan-remote-computer-use.md）。
      ...(options.computerUseCapability !== undefined
        ? { computerUse: options.computerUseCapability }
        : {}),
      // remoteDesktop 同口径：不声明 = 缺键 = 不支持（docs/specs/lan-remote-desktop.md）。
      ...(options.remoteDesktopCapability !== undefined
        ? {
            remoteDesktop:
              typeof options.remoteDesktopCapability === "function"
                ? options.remoteDesktopCapability()
                : options.remoteDesktopCapability,
          }
        : {}),
      // remoteDesktopNative 同口径：不声明 = 缺键 = 旧对端
      //（docs/specs/lan-remote-desktop-native-media.md 版本门 1）。
      ...(options.remoteDesktopNativeCapability !== undefined
        ? {
            remoteDesktopNative:
              typeof options.remoteDesktopNativeCapability === "function"
                ? options.remoteDesktopNativeCapability()
                : options.remoteDesktopNativeCapability,
          }
        : {}),
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

/** /ws/remote-desktop 与 /ws/remote-desktop-media 升级中间件 → 升级处理器之间传递的上下文变量。 */
interface HttpServerVariables {
  remoteDesktopClientId: string;
  remoteDesktopMediaClientId: string;
}

export function createHttpServer(
  services: ServiceCollection,
  port = 3030,
  options: HttpServerOptions = {},
) {
  const app = new Hono<{ Variables: HttpServerVariables }>();
  const { injectWebSocket, upgradeWebSocket } = createNodeWebSocket({ app });
  const hostCapabilities = createHostCapabilityStore();
  // 远程桌面票据与 host capability 分开存放：消费方、绑定关系（clientId）与生命周期都不同。
  const remoteDesktopTickets = createRemoteDesktopTicketStore();
  // 信令票与媒体票各一个 store（接口签名不变，靠实例隔离）：混用同一 store 会让两张票
  // 变得可互换——信令票能升级 media 端点、mediaTicket 能升级信令端点，违反「双票据各自
  // 单次消费、互不能开对方端点」（spec media WS 消息节 + 本任务验收）。
  const remoteDesktopMediaTickets = createRemoteDesktopTicketStore();

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
      // media 端点以会话票据为唯一凭证（spec 原文：票据单次消费、30s、绑定 clientId）：
      // 票据是令牌鉴权 POST 的产物，renderer 无对端长期令牌也不应持有，故豁免全局令牌校验
      // （票据校验在 media 专用中间件里单次消费）。
      if (pathname === "/ws/remote-desktop-media") {
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

  /** 与鉴权同一令牌校验路径：把当前请求出示的令牌解析为已配对客户端 id（解析不到返回 null）。 */
  const resolveLanClientId = (c: Context): string | null => {
    const presented = readPresentedLiteToken(c).token;
    if (!presented || !options.resolveLanClientByToken) {
      return null;
    }
    return options.resolveLanClientByToken(presented);
  };

  // 局域网远程桌面：发起方凭设备令牌请求受控会话，被控端裁决（同意/开关/busy）后
  // 签发一次性票据。请求体无必需字段——发起方身份只来自令牌，避免第二套身份来源。
  app.post("/api/remote-desktop/session", async (c) => {
    const remoteDesktop = options.remoteDesktop;
    if (!remoteDesktop) {
      // 未配置 hooks 的 server（普通 web / 非被控 host）不提供该端点语义。
      return c.json({ error: "Remote desktop is not available" }, 501);
    }
    const clientId = resolveLanClientId(c);
    if (!clientId) {
      // 令牌过了鉴权但解析不到 clientId：无法绑定票据，按未授权拒绝。
      return c.json({ code: "unauthorized" satisfies RemoteDesktopErrorCode }, 401);
    }
    // 画质档位（P1-3）：主控选择随请求下发；缺省/非法值回缺省档（旧主控兼容，
    // 非法值不报错——画质是优化参数，不值得让会话失败）。
    const qualityParam = c.req.query("quality");
    const parsedQuality = qualityParam ? remoteDesktopQualitySchema.safeParse(qualityParam) : null;
    const result = await remoteDesktop.requestSession(
      clientId,
      parsedQuality?.success ? parsedQuality.data : undefined,
    );
    if ("error" in result) {
      return c.json({ code: result.error }, remoteDesktopErrorStatus(result.error));
    }
    // 平铺返回（既有消费者读 issued.ticket / issued.expiresAt，不能整体换成嵌套对象）：
    // 信令票走 ticket、媒体票走 mediaTicket，两张票分别签发、各自单次消费。
    const signaling = remoteDesktopTickets.issue(clientId);
    const media = remoteDesktopMediaTickets.issue(clientId);
    return c.json({
      ticket: signaling.ticket,
      mediaTicket: media.ticket,
      expiresAt: signaling.expiresAt,
    });
  });

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

  // 服务端形态的局域网被连端：设置页状态与动作
  // （docs/specs/lan-host-server-mode.md；动作语义与失败口径见该 spec）。
  if (options.lanAccessHost) {
    const lanAccessHost = options.lanAccessHost;
    app.get("/api/lan/state", (c) => c.json(lanAccessHost.getState()));
    app.post("/api/lan/enable", async (c) => {
      let rawBody: unknown;
      try {
        rawBody = await c.req.json();
      } catch {
        return c.json({ error: "Invalid JSON body" }, 400);
      }
      const parsedBody = lanAccessEnableRequestSchema.safeParse(rawBody);
      if (!parsedBody.success) {
        return c.json({ error: `Invalid request body: ${formatZodError(parsedBody.error)}` }, 400);
      }
      return c.json(await lanAccessHost.setEnabled(parsedBody.data.enabled));
    });
    app.post("/api/lan/pair-code", (c) => c.json(lanAccessHost.createPairCode()));
    app.delete("/api/lan/clients/:clientId", async (c) =>
      c.json(await lanAccessHost.removeClient(c.req.param("clientId"))),
    );
    app.post("/api/lan/reset", async (c) => c.json(await lanAccessHost.resetTokens()));
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

  const upgradeTrustedHostWebSocket = upgradeWebSocket((c) => ({
    onOpen(_event, ws) {
      // 该升级连接对应哪台已配对客户端：cookie/query 令牌 → clientId（与鉴权同一校验路径），
      // 解析成功才挂录制；普通或无 LAN 配置的连接行为不变。
      const presented = readPresentedLiteToken(c).token;
      const clientId =
        presented && options.resolveLanClientByToken
          ? options.resolveLanClientByToken(presented)
          : null;
      setupChannelServer(
        ws.raw as WebSocket,
        services,
        "desktop-continuous",
        clientId && options.onLanClientWorkspace
          ? { clientId, onWorkspace: options.onLanClientWorkspace }
          : undefined,
      );
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

  // 局域网远程桌面信令通道：票据单次消费升级（30s、绑定 clientId），
  // 之后宿主只做 JSON 单帧转发（RemoteDesktopSignalMessage），本模块不解析 SDP/ICE。
  // 失败形态与 /ws/host 一致：返回 JSON 响应、不升级连接。
  const upgradeRemoteDesktopWebSocket = upgradeWebSocket((c) => ({
    onOpen(_event, ws) {
      // LAN 低延迟：显式关 Nagle（服务端 socket 不自带此设置；小控制帧会吃 Nagle×延迟 ACK 抖动）。
      (ws.raw as unknown as { _socket?: { setNoDelay?: (v?: boolean) => void } })._socket?.setNoDelay?.(true);
      const remoteDesktop = options.remoteDesktop;
      const clientId = c.get("remoteDesktopClientId");
      if (!remoteDesktop || !clientId) {
        // 消费与升级之间 host 被撤下（或配置变更）：不留下无归属的信令连接。
        ws.close(4001, "Remote desktop session unavailable");
        return;
      }
      remoteDesktop.onSessionSocket(clientId, ws.raw as WebSocket);
    },
  }));
  app.use("/ws/remote-desktop", async (c, next) => {
    const remoteDesktop = options.remoteDesktop;
    if (!remoteDesktop) {
      return c.json({ error: "Remote desktop is not available" }, 501);
    }
    const ticket = new URL(c.req.url).searchParams.get("ticket") ?? undefined;
    const clientId = resolveLanClientId(c);
    if (!clientId || !remoteDesktopTickets.consume(ticket, clientId)) {
      return c.json({ code: "stale_ticket" satisfies RemoteDesktopErrorCode }, 401);
    }
    c.set("remoteDesktopClientId", clientId);
    await next();
  });
  app.get("/ws/remote-desktop", upgradeRemoteDesktopWebSocket);

  // 局域网远程桌面媒体通道：独立的 mediaTicket store（媒体票据与信令票据互不能开
  // 对方端点），且以票据为唯一凭证——不校验请求方令牌（spec 原文：票据单次消费、
  // 30s、绑定 clientId；renderer 不持有对端长期令牌）。票据在升级中间件单次消费，
  // 升级后的帧交给宿主的 onMediaSocket（本模块不解析二进制/JSON 媒体消息）。
  const upgradeRemoteDesktopMediaWebSocket = upgradeWebSocket((c) => ({
    onOpen(_event, ws) {
      // LAN 低延迟：媒体帧尾包与控制面小帧同样受益于关 Nagle（与信令通道同款设置）。
      (ws.raw as unknown as { _socket?: { setNoDelay?: (v?: boolean) => void } })._socket?.setNoDelay?.(true);
      const remoteDesktop = options.remoteDesktop;
      const clientId = c.get("remoteDesktopMediaClientId");
      if (!remoteDesktop?.onMediaSocket || !clientId) {
        // 消费与升级之间 host 被撤下（或配置变更）：不留下无归属的媒体连接。
        ws.close(4001, "Remote desktop media unavailable");
        return;
      }
      remoteDesktop.onMediaSocket(clientId, ws.raw as WebSocket);
    },
  }));
  app.use("/ws/remote-desktop-media", async (c, next) => {
    const remoteDesktop = options.remoteDesktop;
    if (!remoteDesktop?.onMediaSocket) {
      return c.json({ error: "Remote desktop is not available" }, 501);
    }
    const mediaTicket = new URL(c.req.url).searchParams.get("mediaTicket") ?? undefined;
    // 票据即凭证：不查请求方令牌（全局鉴权对该路径已豁免），由票据原子消费并反解出
    // 签发时绑定的 clientId——票据过期/未知/重放一律 401，双 store 各自独立。
    const clientId = remoteDesktopMediaTickets.consumeAndResolveClientId(mediaTicket);
    if (!clientId) {
      return c.json({ code: "stale_ticket" satisfies RemoteDesktopErrorCode }, 401);
    }
    c.set("remoteDesktopMediaClientId", clientId);
    await next();
  });
  app.get("/ws/remote-desktop-media", upgradeRemoteDesktopMediaWebSocket);

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
