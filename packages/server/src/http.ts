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
  IProviderProvisioningTargetService,
} from "@mode/services";
import {
  formatLogPrefix,
  formatZodError,
  lanAccessEnableRequestSchema,
  lanAccessPairRequestSchema,
  remoteTargetSchema,
  remoteAgentEnrollmentRequestSchema,
  type LanAccessPairingEndpoint,
  type RemoteAgentEnrollmentEndpoint,
  type LanAccessState,
  SERVER_REMOTE_PROTOCOL_VERSION,
  MODE_RPC_HOST_CAPABILITY_HEADER,
  MODE_VERSION,
  type FnosAppUpdateApplyResult,
  type FnosAppUpdateState,
  type ServerRemoteInfo,
  type ServerRemoteWorkspaceInfo,
} from "@mode/shared";
// 远程桌面协议面单独入口（T1），根入口不重导出，避免把桌面链路类型带进所有消费方。
import {
  remoteDesktopCodecSchema,
  remoteDesktopQualitySchema,
  type RemoteDesktopCodec,
  type RemoteDesktopErrorCode,
  type RemoteDesktopQuality,
} from "@mode/shared/remote-desktop";
import { connectRemote, createRemoteBackend, pickRemoteRuntimeEnv } from "./remote/index.js";
import { createRemoteServiceCollection } from "./remote/remoteServiceCollection.js";
import {
  hasRemoteAssetSource,
  REMOTE_ASSET_SOURCE_REQUIRED_MESSAGE,
  resolveServerRemoteAssetDirs,
} from "./remote/remoteAssetSource.js";
import { PendingRemoteConnections } from "./remote/pendingRemoteConnections.js";
import type { LanPeerController } from "./remote/lanPeerController.js";
import type { RemoteDesktopPeerController } from "./remote/remoteDesktopPeerController.js";
import { isDockerAvailable, listDockerContainers } from "./remote/docker-detect.js";
import { listSSHConfigAliasesFromLocalConfig } from "@mode/services/node";
import { createHostCapabilityStore } from "./hostCapability.js";
import { createRemoteDesktopTicketStore } from "./remoteDesktopTicket.js";
import { createLogBundle } from "./logExport.js";
import {
  createTempTextAttachment,
  TEMP_ATTACHMENT_MAX_BYTES,
} from "./tempAttachments.js";

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
    this.inner.registerChannel(channelName, new LanRecordingServerChannel(channel, this.notify));
  }

  ready(): void {
    this.inner.ready?.();
  }
}

function setupChannelServer(
  ws: WebSocket,
  services: ServiceCollection,
  clientMode: "desktop-continuous" | "web-remote-replayable",
  lanRecording?: {
    clientId: string;
    onWorkspace: (clientId: string, workspacePath: string) => void;
  },
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

/** 服务端 fpk 更新器的 HTTP 边界；实现与状态所有者见 appUpdate/manager.ts。 */
export interface FnosAppUpdateEndpoint {
  getState(): FnosAppUpdateState;
  check(): Promise<FnosAppUpdateState>;
  download(): Promise<FnosAppUpdateState>;
  apply(): Promise<FnosAppUpdateApplyResult>;
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
   * SSH/WSL/Docker 的 remote assets 源；缺省读 env。桌面 Host 传入自己解析好的结果，
   * 让手机经 LAN 服务发起的连接与桌面 renderer 使用同一份 CDN/内置 define。
   */
  remoteAssetDirs?: import("./remote/remoteAssetSource.js").ServerRemoteAssetDirs;
  /**
   * 内置公网中继：提供且返回非空时，/api/server-info 下发 publicRelay，
   * 主控据此自动补全公网端点（docs/specs/lan-remote-public-endpoints.md）。
   */
  publicRelay?: () => { baseUrl: string; tunnelId: string } | undefined;
  /** Mode Agent 反向配对：被控端拿桌面生成的配对码登记自己的 MCP 连接信息。 */
  remoteAgentEnrollment?: RemoteAgentEnrollmentEndpoint;
  /**
   * fnOS fpk 在线升级（docs/specs/fnos-feature-parity-and-update.md）。
   * 提供时暴露 /api/app-update/*；普通 Web/桌面部署不传，前端据 enabled=false 隐藏入口。
   */
  appUpdate?: FnosAppUpdateEndpoint;
  /**
   * Web/fpk 日志导出：Host 日志目录 + 额外文件（fpk 的 info.log）。
   * 提供时暴露 GET /api/logs/export；普通桌面壳的 exportLogs 仍走原生保存对话框。
   */
  logsExport?: { logDir: string; extraFiles?: readonly string[] };
  /**
   * Web/fpk 长粘贴文本附件落盘根目录；提供时暴露 POST /api/attachments/temp-text。
   * 桌面端仍走 Tauri 的 createTempTextAttachment 命令。
   */
  tempAttachmentsRoot?: string;
  /**
   * Web/fpk 作为 LAN 主控：配对记录/挂接连接的所有者
   * （docs/specs/web-remote-workspace.md）。提供时暴露 /api/lan/peers/* 与
   * /api/connect-remote-lan；桌面 Host 的 LAN 主控仍走本地 attachment。
   */
  lanPeers?: LanPeerController;
  /**
   * Web/fpk 远程桌面主控中继（docs/specs/web-remote-desktop.md）。
   * 提供时暴露 peer-capability / peer-session 与浏览器信令/媒体中继 WS。
   */
  remoteDesktopPeers?: RemoteDesktopPeerController;
  /**
   * 局域网多客户端：把 /ws/host 升级连接的长期令牌解析为客户端记录 id。
   * 与鉴权走同一令牌校验路径；未提供或解析不到时该连接不参与录制。
   */
  resolveLanClientByToken?: (token: string) => string | null;
  /** 局域网多客户端：该客户端连接上出现携带 workspacePath 的 RPC 时回调（最近在本机打开的目录）。 */
  onLanClientWorkspace?: (clientId: string, workspacePath: string) => void;
  /**
   * 已鉴权的 web/手机远控会话数变化（普通 `/ws` 连接建立/关闭各一次）。
   * 桌面 Host 用它把 `activeWebSessions` 投到 `LanAccessState`，供远控弹窗显示真实在线状态。
   */
  onWebSessionChange?: (delta: 1 | -1) => void;
  /**
   * 服务端形态的局域网被连端（docs/specs/lan-host-server-mode.md）：提供时暴露
   * /api/lan/state 与四个动作端点（开关 / 配对码 / 移除对端 / 重置，均走 /api/* 既有令牌中间件）。
   * 配对端点与逐设备令牌校验分别经 lanPairing / verifyAuthToken 组合提供。
   */
  lanAccessHost?: LanAccessHostEndpoint;
  spaFallback?: boolean;
  staticRoot?: string;
  /**
   * 静态目录的入口文件名（默认 index.html）。桌面 LAN 服务传 `web.html`：
   * 统一前端构建同时产出桌面 `index.html` 与浏览器 `web.html`，浏览器入口不能返回桌面壳。
   */
  staticIndexFile?: string;
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
    codec?: RemoteDesktopCodec,
  ): Promise<{ ok: true } | { error: RemoteDesktopErrorCode }>;
  /**
   * 信令 WS 交接。`session.reissueMediaTicket` 供宿主在会话内处理
   * `{type:"request-media-ticket"}`（媒体 WS 瞬断重连，P2-1）：重签一张媒体票
   * （单次消费、30s、绑定同 clientId）——票据 store 在本模块，宿主不持有。
   */
  onSessionSocket(
    clientId: string,
    socket: WebSocket,
    session?: {
      reissueMediaTicket?: () => { mediaTicket: string; expiresAt: number };
    },
  ): void;
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
    // decoder_unsupported 只在 Web 主控本地产生（浏览器缺 WebCodecs），被控不会返回；
    // 这里按「不支持」归类仅为枚举穷尽。
    case "decoder_unsupported":
      return 501;
    case "internal":
      return 500;
  }
}

/** Web 主控中继的会话请求错误 → HTTP 状态；0 个 code 按内部错误。 */
function rdPeerErrorStatus(
  code: RemoteDesktopErrorCode | undefined,
): 401 | 403 | 409 | 500 | 501 | 504 {
  if (!code) {
    return 500;
  }
  switch (code) {
    case "unauthorized":
    case "stale_ticket":
      return 401;
    case "consent_denied":
    case "disabled":
      return 403;
    case "consent_timeout":
      return 504;
    case "busy":
    case "locally_controlled":
      return 409;
    case "unsupported":
    case "decoder_unsupported":
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

async function resolveServerWorkspaces(
  options: HttpServerOptions,
): Promise<ServerRemoteWorkspaceInfo[]> {
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
  const publicRelay = options.publicRelay?.();
  return {
    serverId: resolveServerId(options),
    ...(options.name?.trim() || readTrimmedEnv("MODE_SERVER_NAME")
      ? { name: options.name?.trim() || readTrimmedEnv("MODE_SERVER_NAME") }
      : {}),
    version: MODE_VERSION,
    protocolVersion: SERVER_REMOTE_PROTOCOL_VERSION,
    authRequired: options.authRequired ?? Boolean(readTrimmedEnv("MODE_SERVER_TOKEN")),
    workspaces: await resolveServerWorkspaces(options),
    ...(publicRelay ? { publicRelay } : {}),
    capabilities: {
      desktopContinuous: true,
      websocketRpc: true,
      // computerUse 只在显式声明时发布：不声明 = 不发键，旧对端与其余消费方行为不变
      // （docs/specs/lan-remote-computer-use.md）。
      ...(options.computerUseCapability !== undefined
        ? { computerUse: options.computerUseCapability }
        : {}),
      ...(options.lanPeers ? { lanPeerController: true } : {}),
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
  staticIndexFile?: string,
): Promise<string | null> {
  const root = resolve(staticRoot);
  const indexFile = staticIndexFile?.trim() || "index.html";
  const normalizedPathname = pathname === "/" ? `/${indexFile}` : pathname;
  const relativePath = decodeURIComponent(normalizedPathname).replace(/^\/+/, "");
  let candidate = resolve(root, relativePath);
  if (!isInsideDirectory(root, candidate)) {
    return null;
  }

  try {
    const candidateStat = await stat(candidate);
    if (candidateStat.isDirectory()) {
      candidate = resolve(candidate, indexFile);
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
  const fallbackFile = resolve(root, indexFile);
  try {
    const indexStat = await stat(fallbackFile);
    return indexStat.isFile() ? fallbackFile : null;
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
  // SSH/WSL/Docker 连接在浏览器接管 `/ws/remote/:id` 之前先落在这里：
  // 60s 未接管即释放，避免 POST 后不接管的连接永久占住 SSH 会话/远端进程。
  const pendingRemoteConnections = new PendingRemoteConnections({ log });
  // 远程桌面票据与 host capability 分开存放：消费方、绑定关系（clientId）与生命周期都不同。
  const remoteDesktopTickets = createRemoteDesktopTicketStore();
  // 信令票与媒体票各一个 store（接口签名不变，靠实例隔离）：混用同一 store 会让两张票
  // 变得可互换——信令票能升级 media 端点、mediaTicket 能升级信令端点，违反「双票据各自
  // 单次消费、互不能开对方端点」（spec media WS 消息节 + 本任务验收）。
  const remoteDesktopMediaTickets = createRemoteDesktopTicketStore();

  // 6 位配对码一旦随公网隧道暴露，必须限速：30 次/分钟让 5 分钟窗口最多 150 次尝试，
  // 远小于 16.7M 的码空间；/api/lan/pair 与 /api/remote-agent/enroll 共用同一个码，
  // 必须一起限（docs/specs/lan-remote-public-endpoints.md 安全边界）。
  const pairingAttempts: number[] = [];
  const PAIRING_ATTEMPT_LIMIT = 30;
  const PAIRING_ATTEMPT_WINDOW_MS = 60_000;
  const allowPairingAttempt = (): boolean => {
    const now = Date.now();
    while (pairingAttempts.length > 0 && now - pairingAttempts[0]! > PAIRING_ATTEMPT_WINDOW_MS) {
      pairingAttempts.shift();
    }
    if (pairingAttempts.length >= PAIRING_ATTEMPT_LIMIT) {
      return false;
    }
    pairingAttempts.push(now);
    return true;
  };

  const authToken = options.authToken?.trim();
  const verifyAuthToken = options.verifyAuthToken;
  if (authToken || verifyAuthToken) {
    app.use("*", async (c, next) => {
      const pathname = new URL(c.req.url).pathname;
      // 配对接口是「登录前引导」：客户端此时没有令牌，凭一次性配对码换取。
      // Mode Agent 反向登记同理：被控端只有桌面生成的配对码，没有长期令牌。
      if (pathname === "/api/lan/pair" || pathname === "/api/remote-agent/enroll") {
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

  // Web 主控的 SSH/Docker 表单数据来源：读服务器本机 SSH 别名与 Docker 容器列表。
  // 探测失败按空结果返回，不阻塞用户手动输入（docs/specs/web-remote-workspace-ssh-docker.md）。
  app.get("/api/remote-environments/ssh-aliases", async (c) => {
    try {
      return c.json({ aliases: await listSSHConfigAliasesFromLocalConfig() });
    } catch {
      return c.json({ aliases: [] });
    }
  });

  app.get("/api/remote-environments/docker", async (c) => {
    try {
      if (!(await isDockerAvailable())) {
        return c.json({ available: false, containers: [] });
      }
      return c.json({ available: true, containers: await listDockerContainers() });
    } catch {
      return c.json({ available: false, containers: [] });
    }
  });

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
    // 编码 codec 偏好（HEVC 选型 2026-10-13）：与画质同款宽容口径——非法/缺省回 h264；
    // 被控无硬件 HEVC 时仍会在 helper 侧回退（解码串由码流自描述兜底，不需要失败）。
    const codecParam = c.req.query("codec");
    const parsedCodec = codecParam ? remoteDesktopCodecSchema.safeParse(codecParam) : null;
    const result = await remoteDesktop.requestSession(
      clientId,
      parsedQuality?.success ? parsedQuality.data : undefined,
      parsedCodec?.success ? parsedCodec.data : undefined,
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
      if (!allowPairingAttempt()) {
        return c.json({ error: "Too many pairing attempts" }, 429);
      }
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
      // 浏览器/手机首次配对：令牌只经 HttpOnly Cookie 落到页面，不要求 JS 读取。
      rememberLiteTokenCookie(c, result.token);
      return c.json(result);
    });
  }

  if (options.remoteAgentEnrollment) {
    const enrollment = options.remoteAgentEnrollment;
    app.post("/api/remote-agent/enroll", async (c) => {
      if (!allowPairingAttempt()) {
        return c.json({ ok: false, error: "rate_limited" }, 429);
      }
      let rawBody: unknown;
      try {
        rawBody = await c.req.json();
      } catch {
        return c.json({ ok: false, error: "invalid_json" }, 400);
      }
      const parsedBody = remoteAgentEnrollmentRequestSchema.safeParse(rawBody);
      if (!parsedBody.success) {
        return c.json(
          { ok: false, error: `invalid_request: ${formatZodError(parsedBody.error)}` },
          400,
        );
      }
      const result = await enrollment.enroll(parsedBody.data);
      return c.json(result, result.ok ? 200 : 401);
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

  // fnOS fpk 在线升级：状态读取与三个动作。所有请求都经过上面的 /api/* 令牌鉴权；
  // 更新器自身只读写 $TRIM_APPDEST，不解析请求体，也不接受客户端传入 URL/版本。
  if (options.appUpdate) {
    const appUpdate = options.appUpdate;
    app.get("/api/app-update/state", (c) => c.json(appUpdate.getState()));
    app.post("/api/app-update/check", async (c) => c.json(await appUpdate.check()));
    app.post("/api/app-update/download", async (c) => c.json(await appUpdate.download(), 202));
    app.post("/api/app-update/apply", async (c) => c.json(await appUpdate.apply()));
  }

  // Web/fpk 作为 LAN 主控（docs/specs/web-remote-workspace.md）。
  // 令牌只留在 credential service；所有响应都不包含 token。
  if (options.lanPeers) {
    const lanPeers = options.lanPeers;
    app.get("/api/lan/peers", async (c) => c.json(await lanPeers.listPeers()));
    app.post("/api/lan/peers/pair", async (c) => {
      let rawBody: unknown;
      try {
        rawBody = await c.req.json();
      } catch {
        return c.json({ error: "invalid_json" }, 400);
      }
      const body = (rawBody ?? {}) as Record<string, unknown>;
      const host = typeof body.host === "string" ? body.host.trim() : "";
      const port = typeof body.port === "number" ? body.port : 0;
      const code = typeof body.code === "string" ? body.code.trim() : "";
      if (!host || !Number.isInteger(port) || port <= 0 || port > 65535 || !code) {
        return c.json({ error: "invalid_pair_request" }, 400);
      }
      try {
        return c.json(
          await lanPeers.pairPeer({
            host,
            port,
            code,
            ...(typeof body.label === "string" ? { label: body.label } : {}),
            ...(body.scheme === "https" || body.scheme === "http"
              ? { scheme: body.scheme }
              : {}),
            ...(typeof body.path === "string" ? { path: body.path } : {}),
          }),
        );
      } catch (error) {
        return c.json(
          { error: error instanceof Error ? error.message : String(error) },
          502,
        );
      }
    });
    app.post("/api/lan/peers/discover", async (c) => c.json(await lanPeers.discoverPeers()));
    app.get("/api/lan/peers/:serverId/workspaces", async (c) => {
      try {
        return c.json(await lanPeers.getPeerWorkspaces(c.req.param("serverId")));
      } catch (error) {
        return c.json(
          { error: error instanceof Error ? error.message : String(error) },
          502,
        );
      }
    });
    app.post("/api/lan/peers/:serverId/rename", async (c) => {
      let rawBody: unknown;
      try {
        rawBody = await c.req.json();
      } catch {
        return c.json({ error: "invalid_json" }, 400);
      }
      const name = (rawBody as { name?: unknown } | null)?.name;
      if (typeof name !== "string") {
        return c.json({ error: "invalid_name" }, 400);
      }
      await lanPeers.renamePeer(c.req.param("serverId"), name);
      return c.json({ ok: true });
    });
    app.delete("/api/lan/peers/:serverId", async (c) => {
      await lanPeers.removePeer(c.req.param("serverId"));
      return c.json({ ok: true });
    });
    app.post("/api/connect-remote-lan", async (c) => {
      let rawBody: unknown;
      try {
        rawBody = await c.req.json();
      } catch {
        return c.json({ error: "invalid_json" }, 400);
      }
      const parsed = remoteTargetSchema.safeParse(rawBody);
      if (!parsed.success || parsed.data.kind !== "lan") {
        return c.json({ error: "invalid_lan_target" }, 400);
      }
      try {
        return c.json(await lanPeers.connectRemote(parsed.data));
      } catch (error) {
        return c.json(
          { error: error instanceof Error ? error.message : String(error) },
          502,
        );
      }
    });
    app.post("/api/remote-workspace/:id/release", async (c) => {
      await lanPeers.releaseRemote(c.req.param("id"));
      return c.json({ ok: true });
    });
  }

  // Web/fpk 远程桌面主控中继（docs/specs/web-remote-desktop.md）。
  if (options.remoteDesktopPeers) {
    const rdPeers = options.remoteDesktopPeers;
    app.get("/api/remote-desktop/peer-capability", async (c) => {
      const serverId = c.req.query("serverId")?.trim();
      if (!serverId) {
        return c.json({ error: "missing_server_id" }, 400);
      }
      try {
        return c.json(await rdPeers.probe(serverId));
      } catch {
        return c.json({ online: false, remoteDesktop: false, remoteDesktopNative: false });
      }
    });
    app.post("/api/remote-desktop/peer-session", async (c) => {
      let rawBody: unknown;
      try {
        rawBody = await c.req.json();
      } catch {
        return c.json({ error: "invalid_json" }, 400);
      }
      const body = (rawBody ?? {}) as Record<string, unknown>;
      const serverId = typeof body.serverId === "string" ? body.serverId.trim() : "";
      if (!serverId) {
        return c.json({ error: "missing_server_id" }, 400);
      }
      const quality = remoteDesktopQualitySchema.safeParse(body.quality);
      const codec = remoteDesktopCodecSchema.safeParse(body.codec);
      const result = await rdPeers.start(
        serverId,
        quality.success ? quality.data : undefined,
        codec.success ? codec.data : undefined,
      );
      if (!result.ok) {
        return c.json(result, rdPeerErrorStatus(result.errorCode));
      }
      return c.json(result);
    });
    app.post("/api/remote-desktop/peer-session/:id/end", async (c) => {
      let reason: "user-end" | "error" = "user-end";
      try {
        const body = (await c.req.json()) as { reason?: unknown };
        if (body?.reason === "error") {
          reason = "error";
        }
      } catch {
        // 无 body / 非 JSON：按默认 user-end。
      }
      rdPeers.end(c.req.param("id"), reason);
      return c.json({ ok: true });
    });
    app.get(
      "/ws/remote-desktop-peer/:id/signal",
      upgradeWebSocket((c) => {
        const id = c.req.param("id");
        return {
          onOpen(_event, ws) {
            rdPeers.attachBrowserSignal(id ?? "", ws.raw as WebSocket);
          },
        };
      }),
    );
    app.get(
      "/ws/remote-desktop-peer/:id/media",
      upgradeWebSocket((c) => {
        const id = c.req.param("id");
        return {
          onOpen(_event, ws) {
            rdPeers.attachBrowserMedia(id ?? "", ws.raw as WebSocket);
          },
        };
      }),
    );
  }

  // Web/fpk 长粘贴文本：落成服务端临时文件，路径随后由 Host fileService 读取。
  if (options.tempAttachmentsRoot) {
    const tempAttachmentsRoot = options.tempAttachmentsRoot;
    app.post("/api/attachments/temp-text", async (c) => {
      let rawBody: unknown;
      try {
        rawBody = await c.req.json();
      } catch {
        return c.json({ error: "invalid_json" }, 400);
      }
      if (
        !rawBody ||
        typeof rawBody !== "object" ||
        typeof (rawBody as { text?: unknown }).text !== "string"
      ) {
        return c.json({ error: "invalid_request" }, 400);
      }
      const body = rawBody as { text: string; filename?: unknown };
      if (Buffer.byteLength(body.text, "utf8") > TEMP_ATTACHMENT_MAX_BYTES) {
        return c.json({ error: "payload_too_large" }, 413);
      }
      const result = await createTempTextAttachment(tempAttachmentsRoot, {
        text: body.text,
        ...(typeof body.filename === "string" ? { filename: body.filename } : {}),
      });
      return c.json(result);
    });
  }

  // Web/fpk 日志导出：直接返回文本包下载（浏览器端由 Web 平台触发）。
  if (options.logsExport) {
    const logsExport = options.logsExport;
    app.get("/api/logs/export", async (c) => {
      const result = await createLogBundle(logsExport);
      if (!result) {
        return c.json({ error: "no_logs" }, 404);
      }
      return c.body(result.bundle, 200, {
        "content-type": "text/plain; charset=utf-8",
        "content-disposition": `attachment; filename="${result.fileName}"`,
        "cache-control": "no-store",
      });
    });
  }

  // 普通 `/ws` 永远是 terminal-client；浏览器/任意客户端设置旧 mode header
  // 都不能再把自己提升为 trusted host。
  app.get(
    "/ws",
    upgradeWebSocket(() => ({
      onOpen(_event, ws) {
        // /ws 走全局令牌鉴权（isTokenProtectedPath），能升级成功即代表一台已配对客户端在线。
        options.onWebSessionChange?.(1);
        setupChannelServer(ws.raw as WebSocket, services, "web-remote-replayable");
      },
      onClose() {
        options.onWebSessionChange?.(-1);
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
      (
        ws.raw as unknown as { _socket?: { setNoDelay?: (v?: boolean) => void } }
      )._socket?.setNoDelay?.(true);
      const remoteDesktop = options.remoteDesktop;
      const clientId = c.get("remoteDesktopClientId");
      if (!remoteDesktop || !clientId) {
        // 消费与升级之间 host 被撤下（或配置变更）：不留下无归属的信令连接。
        ws.close(4001, "Remote desktop session unavailable");
        return;
      }
      remoteDesktop.onSessionSocket(clientId, ws.raw as WebSocket, {
        // 媒体 WS 瞬断重连（P2-1）：票据 store 在本模块，重签能力随会话上下文交给宿主；
        // 重签出的票与 POST 签发的票同构（单次消费、30s、绑定同 clientId）。
        reissueMediaTicket: () => {
          const issued = remoteDesktopMediaTickets.issue(clientId);
          return { mediaTicket: issued.ticket, expiresAt: issued.expiresAt };
        },
      });
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
      (
        ws.raw as unknown as { _socket?: { setNoDelay?: (v?: boolean) => void } }
      )._socket?.setNoDelay?.(true);
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

  // Web 模式下发起远程连接（SSH / WSL / Docker）
  app.post("/api/connect-remote", async (c) => {
    const rawBody = await c.req.json();
    const parsedBody = remoteTargetSchema.safeParse(rawBody);
    if (!parsedBody.success) {
      return c.json({ error: `Invalid request body: ${formatZodError(parsedBody.error)}` }, 400);
    }
    const body = parsedBody.data;
    if (body.kind === "lan") {
      // LAN 目标必须在发起端本地做令牌/端点解析；服务端不接收任意 URL + 令牌组合。
      return c.json({ error: "LAN targets must use /api/connect-remote-lan" }, 400);
    }

    // 资产源必须在建 backend 之前检查：否则会先连上 SSH 再在部署阶段失败。
    const assetDirs = options.remoteAssetDirs ?? resolveServerRemoteAssetDirs();
    if (!hasRemoteAssetSource(assetDirs)) {
      return c.json(
        { error: "remote_assets_unavailable", message: REMOTE_ASSET_SOURCE_REQUIRED_MESSAGE },
        409,
      );
    }

    let connection: Awaited<ReturnType<typeof connectRemote>> | null = null;
    try {
      const backend = await createRemoteBackend(body);
      connection = await connectRemote(backend, {
        ...assetDirs,
        appVersion: MODE_VERSION,
        remoteRuntimeEnv: pickRemoteRuntimeEnv(process.env),
      });
      const added = pendingRemoteConnections.add(connection);
      if (!added.ok) {
        connection.dispose();
        return c.json({ error: added.error }, 503);
      }
      return c.json({ id: added.id });
    } catch (err: unknown) {
      connection?.dispose();
      const message = err instanceof Error ? err.message : String(err);
      return c.json({ error: message }, 500);
    }
  });

  // 远程连接的 WebSocket 端点，将远程 services 桥接给浏览器
  app.get(
    "/ws/remote/:id",
    upgradeWebSocket((c) => {
      const id = c.req.param("id");
      let disposeConnection: (() => void) | null = null;
      return {
        onOpen(_event, ws) {
          if (!id) {
            ws.close(4000, "Missing remote connection id");
            return;
          }
          // SSH/WSL/Docker 的 stdio connection 与 LAN attach 的 connection 各自一个来源；
          // 浏览器只拿到一个 id，因此两个来源都查一次，取出即从原来源删除。
          const stdioConnection = pendingRemoteConnections.take(id);
          if (stdioConnection) {
            disposeConnection = () => stdioConnection.dispose();
            setupChannelServer(
              ws.raw as WebSocket,
              createRemoteServiceCollection(stdioConnection.services),
              "web-remote-replayable",
            );
            return;
          }
          const lanConnection =
            options.lanPeers?.takeRemoteConnection(id, () => {
              // 对端先断：主动关掉浏览器 WS，让页面走正常断线收口，而不是停在半死服务上。
              try {
                ws.close(4001, "Remote disconnected");
              } catch {
                // WS 可能已经在关闭中。
              }
            }) ?? null;
          if (lanConnection) {
            disposeConnection = () => void lanConnection.dispose();
            setupChannelServer(
              ws.raw as WebSocket,
              createRemoteServiceCollection(lanConnection.services),
              "web-remote-replayable",
            );
            return;
          }
          ws.close(4004, "Remote connection not found");
        },
        onClose() {
          // 浏览器刷新/关 tab 即释放 server 侧 remote attachment；重复 dispose 幂等。
          disposeConnection?.();
          disposeConnection = null;
        },
      };
    }),
  );

  if (options.staticRoot?.trim()) {
    const staticRoot = options.staticRoot.trim();
    app.get("*", async (c) => {
      const pathname = new URL(c.req.url).pathname;
      const filePath = await resolveStaticFile(
        staticRoot,
        pathname,
        options.spaFallback ?? true,
        options.staticIndexFile,
      );
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

  // 服务关闭时释放尚未被浏览器接管的 SSH/Docker 连接；已接管的由各自 WS close 负责。
  server.on("close", () => pendingRemoteConnections.disposeAll());

  return server;
}
