import { Emitter, SocketProtocol, VSBuffer, type ISocket } from "@mode/rpc";
import { connectViaProtocol } from "@mode/client";
import { hostname } from "node:os";
import {
  buildLanPeerTokenKey,
  buildLanPeerMetaKey,
  dedupeLanPeerEndpoints,
  parseLanPeerMeta,
  parseLanPublicBaseUrl,
  serverRemoteInfoSchema,
  sortLanPeerEndpoints,
  MODE_RPC_HOST_CAPABILITY_HEADER,
  type LanAccessPairResult,
  type LanAccessState,
  type LanPeerEndpoint,
  type ServerRemoteInfo,
} from "@mode/shared";
import type { ICredentialService, IServiceAccessor } from "@mode/services";
import { WebSocket, type RawData } from "ws";

/**
 * 局域网客户端（Host 侧）：配对换令牌、按令牌挂接对端的受信 RPC 通道。
 *
 * 与 SSH/WSL/Docker 不同，这里不部署也不启动任何东西：对端是「已经在跑的 Mode 实例」，
 * 因此跳过 upload/deploy/hello 握手，直接用 HTTP(/api/server-info、/api/rpc-host-capability)
 * + WebSocket(/ws/host) 复用同一套 SocketProtocol → ChannelClient → RemoteServiceAccess。
 */

const LAN_PAIR_TIMEOUT_MS = 10_000;
const LAN_INFO_TIMEOUT_MS = 10_000;
/**
 * WS 存活探测周期。网络切换 / 休眠恢复可能留下半开连接：TCP 不发 FIN/RST 时
 * `close` 不会触发，UI 会一直停在「已连接」。ping 未在下一周期前收到 pong 即终止
 * 连接，让既有 close 语义接管；不给业务 RPC 增加超时。
 */
const LAN_WS_HEARTBEAT_INTERVAL_MS = 30_000;

/** WebSocket 协议由 HTTP 端点协议推导：https → wss（公网必须走 TLS）。 */
export function toWebSocketScheme(scheme: "http" | "https"): "ws" | "wss" {
  return scheme === "https" ? "wss" : "ws";
}

export function buildLanHttpBase(
  host: string,
  port: number,
  scheme: "http" | "https" = "http",
): string {
  return `${scheme}://${host}:${port}`;
}

export function buildLanWsUrl(
  host: string,
  port: number,
  scheme: "http" | "https" = "http",
): string {
  return `${toWebSocketScheme(scheme)}://${host}:${port}/ws/host`;
}

/**
 * 端点请求错误：`retryable` 决定是否切换下一个端点。只有传输失败/超时/5xx/404/
 * 协议不符可重试；401/403 等对端级结果不重试（同一令牌在另一个端点不会更好）。
 */
export class LanPeerRequestError extends Error {
  constructor(
    message: string,
    readonly retryable: boolean,
    readonly code?: "unauthorized" | "internal",
  ) {
    super(message);
  }
}

export function isRetryableLanPeerError(error: unknown): boolean {
  return error instanceof LanPeerRequestError && error.retryable;
}

async function fetchWithTimeout(
  url: string,
  init: RequestInit,
  timeoutMs: number,
): Promise<Response> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    return await fetch(url, { ...init, signal: controller.signal });
  } finally {
    clearTimeout(timer);
  }
}

async function fetchPeerResponse(
  url: string,
  init: RequestInit,
  timeoutMs: number,
): Promise<Response> {
  try {
    return await fetchWithTimeout(url, init, timeoutMs);
  } catch (error) {
    const timedOut = error instanceof Error && error.name === "AbortError";
    throw new LanPeerRequestError(
      timedOut
        ? `连接对端超时（${timeoutMs}ms）`
        : `连接对端失败：${error instanceof Error ? error.message : String(error)}`,
      true,
    );
  }
}

/** 读取对端信息（包含工作区列表）；协议版本不匹配时抛出可读错误。 */
export async function fetchLanPeerInfo(params: {
  scheme?: "http" | "https";
  host: string;
  port: number;
  token?: string;
  /** 端点探测预算；缺省 10s（与既有 LAN 行为一致）。 */
  timeoutMs?: number;
}): Promise<ServerRemoteInfo> {
  const url = new URL(
    "/api/server-info",
    buildLanHttpBase(params.host, params.port, params.scheme ?? "http"),
  );
  if (params.token) {
    url.searchParams.set("token", params.token);
  }
  const response = await fetchPeerResponse(
    url.toString(),
    { method: "GET" },
    params.timeoutMs ?? LAN_INFO_TIMEOUT_MS,
  );
  if (response.status === 401 || response.status === 403) {
    throw new LanPeerRequestError(
      response.status === 401
        ? "对端拒绝了本次访问：令牌无效或已被重置，需要重新配对"
        : "对端拒绝了本次访问（HTTP 403）",
      false,
      "unauthorized",
    );
  }
  if (!response.ok) {
    // 404/5xx 通常来自反向代理或隧道（路由未就绪）；换下一个端点有意义。
    const retryable =
      response.status === 404 ||
      response.status === 502 ||
      response.status === 503 ||
      response.status === 504 ||
      response.status >= 500;
    throw new LanPeerRequestError(`读取对端信息失败（HTTP ${response.status}）`, retryable);
  }
  let payload: unknown;
  try {
    payload = await response.json();
  } catch {
    // 反代/验证页返回 HTML 等：视为端点不可用，允许回退到另一个端点。
    throw new LanPeerRequestError("对端返回的信息不是合法 JSON", true);
  }
  const parsed = serverRemoteInfoSchema.safeParse(payload);
  if (!parsed.success) {
    throw new LanPeerRequestError(
      "对端返回的信息不符合局域网连接协议，请确认两端 Mode 版本一致",
      true,
    );
  }
  return parsed.data;
}

/** 用一次性配对码换取该设备专属长期令牌，并写入凭据服务。 */
export async function pairLanPeer(params: {
  credentials: ICredentialService;
  host: string;
  port: number;
  code: string;
  label?: string;
  /** 主端点协议；缺省 http。https 用于公网地址配对。 */
  scheme?: "http" | "https";
  /** 可选：配对成功后一并记录的公网 HTTPS 基础地址。 */
  publicBaseUrl?: string;
}): Promise<LanAccessPairResult> {
  const scheme = params.scheme === "https" ? "https" : "http";
  const host = params.host.trim();
  if (!host) {
    throw new Error("缺少对端地址");
  }
  if (!Number.isInteger(params.port) || params.port <= 0 || params.port > 65535) {
    throw new Error("对端端口无效");
  }
  const publicBaseUrl = params.publicBaseUrl?.trim() ?? "";
  const publicEndpoint = publicBaseUrl ? parseLanPublicBaseUrl(publicBaseUrl) : null;
  if (publicBaseUrl && !publicEndpoint) {
    throw new Error("公网地址必须是 HTTPS，且不能带路径、查询参数或账号信息");
  }
  const primaryEndpoint: LanPeerEndpoint = {
    scheme,
    host,
    port: params.port,
    kind: scheme === "https" ? "public" : "lan",
  };
  // 端点顺序是连接优先级：LAN 在前、公网在后；同类保持调用方顺序。
  const newEndpoints = [primaryEndpoint, ...(publicEndpoint ? [publicEndpoint] : [])];
  const response = await fetchWithTimeout(
    new URL("/api/lan/pair", buildLanHttpBase(host, params.port, scheme)).toString(),
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        code: params.code.trim().toUpperCase(),
        // label = 本机身份（对端「配对我的设备」按它显示设备名）：显式传入优先
        //（验证脚本在用），缺省兜底本机主机名——向导不再传 navigator.platform
        //（Windows 上是 "Win32"，对端列表就会显示 win32）。
        label: params.label?.trim() || hostname(),
      }),
    },
    LAN_PAIR_TIMEOUT_MS,
  );
  if (response.status === 401) {
    throw new Error("配对码无效或已过期，请在对端重新生成");
  }
  if (!response.ok) {
    throw new Error(`配对失败（HTTP ${response.status}）`);
  }
  const payload = (await response.json()) as Partial<LanAccessPairResult>;
  if (typeof payload.token !== "string" || !payload.token) {
    throw new Error("配对响应缺少令牌");
  }
  const result: LanAccessPairResult = {
    token: payload.token,
    serverId: typeof payload.serverId === "string" && payload.serverId ? payload.serverId : "",
    ...(typeof payload.name === "string" && payload.name ? { name: payload.name } : {}),
  };
  if (result.serverId) {
    await params.credentials.save(buildLanPeerTokenKey(result.serverId), result.token);
    // 重复配对同一 serverId 时合并端点：本次配对的端点优先作为主地址；已有 LAN 端点
    // 保留在后（换网络后旧地址可能失效，但探测会跳过它）；新配对带公网端点时替换旧
    // 公网端点，否则保留已配置的公网地址。这样两个方向的重复配对都不会丢入口。
    const existingMeta = parseLanPeerMeta(
      await params.credentials.load(buildLanPeerMetaKey(result.serverId)),
    );
    const existingEndpoints = existingMeta?.endpoints ?? [];
    const newPublic = newEndpoints.filter((endpoint) => endpoint.kind === "public");
    const merged = sortLanPeerEndpoints(
      dedupeLanPeerEndpoints([
        ...newEndpoints,
        ...existingEndpoints.filter((endpoint) => endpoint.kind === "lan"),
        ...(newPublic.length > 0
          ? []
          : existingEndpoints.filter((endpoint) => endpoint.kind === "public")),
      ]),
    );
    // 配对元数据与令牌同生命周期：设置页「我配对的对端」凭它列出地址与名称，
    // 枚举侧只读键名/元数据，不接触令牌本体。对端未自报名称时保留本机重命名。
    const name = result.name ?? existingMeta?.name;
    await params.credentials.save(
      buildLanPeerMetaKey(result.serverId),
      JSON.stringify({
        endpoints: merged,
        ...(name ? { name } : {}),
        pairedAt: Date.now(),
      }),
    );
  }
  return result;
}

/** 历史重连：按 serverId 取回之前配对得到的令牌。 */
export async function loadLanPeerToken(
  credentials: ICredentialService,
  serverId: string | undefined,
): Promise<string | null> {
  if (!serverId?.trim()) {
    return null;
  }
  return await credentials.load(buildLanPeerTokenKey(serverId.trim()));
}

/**
 * 读取已配对对端的端点与令牌（令牌不出 Host）。端点顺序即连接优先级：
 * LAN 在前、公网在后；旧记录由 parseLanPeerMeta 迁移为单个 LAN 端点。
 */
export async function readLanPeerEndpoints(
  credentials: ICredentialService,
  serverId: string,
): Promise<{ endpoints: LanPeerEndpoint[]; token?: string } | null> {
  const trimmed = serverId.trim();
  if (!trimmed) {
    return null;
  }
  const meta = parseLanPeerMeta(await credentials.load(buildLanPeerMetaKey(trimmed)));
  if (!meta) {
    return null;
  }
  const token = (await credentials.load(buildLanPeerTokenKey(trimmed)))?.trim();
  return { endpoints: meta.endpoints, ...(token ? { token } : {}) };
}

function wrapNodeWebSocket(ws: WebSocket): ISocket {
  const onData = new Emitter<VSBuffer>();
  const onClose = new Emitter<void>();
  const onEnd = new Emitter<void>();
  let heartbeat: ReturnType<typeof setInterval> | undefined;
  let alive = true;
  let closed = false;
  const stopHeartbeat = (): void => {
    if (heartbeat !== undefined) {
      clearInterval(heartbeat);
      heartbeat = undefined;
    }
  };
  // close / error / 心跳超时可能先后到达：关闭信号只发一次，避免 registry 重复收尸。
  const emitClose = (): void => {
    if (closed) {
      return;
    }
    closed = true;
    stopHeartbeat();
    onClose.fire();
    onEnd.fire();
  };
  ws.on("message", (data: RawData, isBinary: boolean) => {
    if (!isBinary) {
      // 该通道只承载二进制 RPC 帧；文本帧一律忽略，避免把协议噪声当帧喂给 ChannelClient。
      return;
    }
    const bytes = Array.isArray(data)
      ? Buffer.concat(data)
      : Buffer.isBuffer(data)
        ? data
        : Buffer.from(data as ArrayBuffer);
    onData.fire(VSBuffer.wrap(new Uint8Array(bytes)));
  });
  ws.on("open", () => {
    alive = true;
    heartbeat = setInterval(() => {
      if (ws.readyState !== WebSocket.OPEN) {
        stopHeartbeat();
        return;
      }
      if (!alive) {
        // 上一周期没有 pong：判定半开，主动 terminate 触发 close。
        emitClose();
        ws.terminate();
        return;
      }
      alive = false;
      try {
        ws.ping();
      } catch {
        emitClose();
        ws.terminate();
      }
    }, LAN_WS_HEARTBEAT_INTERVAL_MS);
    // 心跳不能单独撑住进程退出（dispose 会 clearInterval；unref 再兜一层）。
    heartbeat.unref?.();
  });
  ws.on("pong", () => {
    alive = true;
  });
  ws.on("close", emitClose);
  ws.on("error", emitClose);
  return {
    onData: onData.event,
    onClose: onClose.event,
    onEnd: onEnd.event,
    write(buffer: VSBuffer) {
      if (ws.readyState === WebSocket.OPEN) {
        ws.send(buffer.buffer, { binary: true });
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

export interface LanRemoteConnection {
  services: IServiceAccessor;
  info: ServerRemoteInfo;
  dispose: () => Promise<void>;
}

/**
 * 挂接对端：申请一次性 host capability（提权票据）→ 以受信中继身份打开 /ws/host。
 * 令牌是长期凭据（配对时下发），capability 只是本次连接的提权声明。
 */
export async function attachLanRemoteConnection(params: {
  scheme?: "http" | "https";
  host: string;
  port: number;
  token: string;
  signal?: AbortSignal;
  /** 对端通道断开（含进程退出、网络中断）时回调：与 SSH backend.onDidDisconnect 语义一致。 */
  onClose?: () => void;
}): Promise<LanRemoteConnection> {
  const scheme = params.scheme ?? "http";
  const info = await fetchLanPeerInfo({
    scheme,
    host: params.host,
    port: params.port,
    token: params.token,
  });
  if (params.signal?.aborted) {
    throw new Error("远程连接已取消");
  }

  const capabilityResponse = await fetchWithTimeout(
    new URL(
      "/api/rpc-host-capability",
      buildLanHttpBase(params.host, params.port, scheme),
    ).toString(),
    {
      method: "POST",
      // 服务端只从 ?token= 或 mode_lite_token Cookie 取令牌；这里用 Cookie，避免令牌进 URL/日志。
      headers: { cookie: `mode_lite_token=${encodeURIComponent(params.token)}` },
    },
    LAN_INFO_TIMEOUT_MS,
  );
  if (!capabilityResponse.ok) {
    throw new Error(`申请对端连接票据失败（HTTP ${capabilityResponse.status}）`);
  }
  const capabilityPayload = (await capabilityResponse.json()) as { capability?: unknown };
  if (typeof capabilityPayload.capability !== "string" || !capabilityPayload.capability) {
    throw new Error("对端未返回连接票据");
  }

  const ws = new WebSocket(buildLanWsUrl(params.host, params.port, scheme), {
    headers: {
      [MODE_RPC_HOST_CAPABILITY_HEADER]: capabilityPayload.capability,
      cookie: `mode_lite_token=${encodeURIComponent(params.token)}`,
    },
  });
  // 修复依据：服务端 ChannelServer 只在建连瞬间发一帧 Initialize，且常与握手回包
  // 同批到达——Node ws 会在同一回调里先 emit open、紧接着处理该帧。若等 open 之后
  // 再挂 message 监听/建协议层，Initialize 会被静默丢弃且不重发，ChannelClient 永久
  // 停在 Uninitialized，所有 RPC 无报错无日志地排队（目录浏览器卡「加载中…」）。
  // 因此 wrap、SocketProtocol 与 ChannelClient 必须在 await open 之前同步构造。
  const services = connectViaProtocol(new SocketProtocol(wrapNodeWebSocket(ws)));
  await new Promise<void>((resolve, reject) => {
    const onOpen = () => {
      ws.off("error", onError);
      resolve();
    };
    const onError = (error: Error) => {
      ws.off("open", onOpen);
      reject(new Error(`连接对端通道失败：${error.message}`));
    };
    ws.once("open", onOpen);
    ws.once("error", onError);
  });

  // 半开连接补偿：ws 关闭/出错都会触发 onClose，让 registry 能立刻把 session 置为断开。
  ws.once("close", () => params.onClose?.());
  ws.once("error", () => params.onClose?.());

  return {
    services,
    info,
    async dispose() {
      try {
        ws.close();
      } catch {
        // 已关闭时忽略。
      }
    },
  };
}

/** 服务端设置卡展示用：把状态转成一行可读地址（无地址时给出提示）。 */
export function formatLanAccessAddresses(state: LanAccessState): string {
  if (!state.enabled || !state.port) {
    return "";
  }
  return state.addresses.length > 0 ? state.addresses.join("、") : `0.0.0.0:${state.port}`;
}
