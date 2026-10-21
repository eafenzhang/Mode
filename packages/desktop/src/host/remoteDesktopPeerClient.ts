/**
 * 主控方向的对端操作（host 侧）：对端能力位探测与受控会话请求。
 *
 * 为什么放在 host 而不在 main：对端长期令牌保存在凭据服务（`lan:peer:<serverId>:token`），
 * host 是本机 LAN 客户端凭据的既有持有者；令牌只随本模块的出站请求交给对端
 * （POST 与 WS 升级都用 `?token=` query，与服务端 `readPresentedLiteToken` 的读取口同形），
 * 不写日志、不进 renderer。
 *
 * 公网端点（docs/specs/lan-remote-public-endpoints.md）：一个已配对设备可同时保存 LAN
 * （http）与公网（https）端点；探测按公网 → LAN 回退，会话先探测选出可达端点再 POST。
 * https 端点的信令/媒体 WebSocket 必须用 wss，禁止降级到明文。
 */
import type { LanPeerEndpoint } from "@mode/shared";
import {
  DEFAULT_REMOTE_DESKTOP_CODEC,
  DEFAULT_REMOTE_DESKTOP_QUALITY,
  type RemoteDesktopCodec,
  type RemoteDesktopErrorCode,
  type RemoteDesktopQuality,
} from "@mode/shared/remote-desktop";
import type { ICredentialService } from "@mode/services";
import { fetchLanPeerInfo, readLanPeerEndpoints } from "./lanRemoteAttach.js";
import {
  buildLanApiUrl,
  buildLanHttpBase,
  isRetryableLanPeerError,
  LanPeerRequestError,
  toWebSocketScheme,
} from "./lanPeerTransport.js";

/**
 * 会话 POST 阻塞在被控同意流（60s 同意窗 + 75s 兜底，remoteDesktopServer.ts），
 * 主控预算必须覆盖整个等待期，否则用户没来得及点「允许」就被归因 internal（spec:
 * lan-remote-desktop 事件顺序 ② 主控侧等待预算 90s）。能力位探测不走本函数，
 * 由 fetchLanPeerInfo 自带短超时。
 */
const SESSION_REQUEST_TIMEOUT_MS = 90_000;
/** 多端点时 LAN 回退探测预算：公网不可达时不长时间卡在局域网地址上。 */
const LAN_FALLBACK_PROBE_TIMEOUT_MS = 2_500;
/** 多端点时公网探测预算：覆盖 TLS 握手与反向隧道冷启动。 */
const PUBLIC_PROBE_TIMEOUT_MS = 8_000;

export interface RemoteDesktopPeerInfo {
  online: boolean;
  remoteDesktop: boolean;
  /** 原生媒体面版本门（纯直替）：旧对端缺键 = false（docs/specs/lan-remote-desktop-native-media.md）。 */
  remoteDesktopNative: boolean;
  name?: string;
  /** 本次命中的端点类型；离线时缺省。 */
  endpointKind?: "lan" | "public";
  endpointHost?: string;
}

interface PeerTarget {
  endpoints: LanPeerEndpoint[];
  token: string;
}

function offlineProbe(): RemoteDesktopPeerInfo {
  return { online: false, remoteDesktop: false, remoteDesktopNative: false };
}

function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * 读取端点 + 令牌。返回 null = 没有配对元数据；`token` 为空 = 元数据在但令牌缺失。
 * 不在本模块做“上次成功的端点”缓存：网络切换后必须重新按顺序探测。
 */
async function readPeerTarget(
  credentials: ICredentialService,
  serverId: string,
): Promise<PeerTarget | null> {
  const target = await readLanPeerEndpoints(credentials, serverId);
  if (!target) {
    return null;
  }
  return target.token
    ? { endpoints: target.endpoints, token: target.token }
    : { endpoints: target.endpoints, token: "" };
}

function endpointProbeTimeoutMs(
  endpoint: LanPeerEndpoint,
  endpointCount: number,
): number | undefined {
  if (endpointCount <= 1) {
    // 单端点沿用既有 10s 预算（fetchLanPeerInfo 缺省值）。
    return undefined;
  }
  return endpoint.kind === "lan" ? LAN_FALLBACK_PROBE_TIMEOUT_MS : PUBLIC_PROBE_TIMEOUT_MS;
}

function endpointRequest(endpoint: LanPeerEndpoint): {
  scheme: "http" | "https";
  host: string;
  port: number;
  path?: string;
} {
  return {
    scheme: endpoint.scheme,
    host: endpoint.host,
    port: endpoint.port,
    ...(endpoint.path ? { path: endpoint.path } : {}),
  };
}

/**
 * 探测：公网优先、LAN 回退。401/403 等对端级错误立即收口；只有传输失败/超时/5xx/404/
 * 协议不符才尝试下一个端点（同一令牌在另一个端点不会更好）。
 */
export async function probeRemoteDesktopPeer(
  credentials: ICredentialService,
  serverId: string,
): Promise<RemoteDesktopPeerInfo> {
  const target = await readPeerTarget(credentials, serverId);
  if (!target || !target.token) {
    return offlineProbe();
  }
  for (const endpoint of target.endpoints) {
    try {
      const timeoutMs = endpointProbeTimeoutMs(endpoint, target.endpoints.length);
      const info = await fetchLanPeerInfo({
        ...endpointRequest(endpoint),
        token: target.token,
        ...(timeoutMs !== undefined ? { timeoutMs } : {}),
      });
      return {
        online: true,
        // 缺键 = 旧对端不支持（docs/specs/lan-remote-desktop.md 产品规则 5）。
        remoteDesktop: info.capabilities?.remoteDesktop === true,
        // 原生媒体面版本门：同样缺键 = 旧对端（纯直替，缺 native = 不进会话）。
        remoteDesktopNative: info.capabilities?.remoteDesktopNative === true,
        ...(info.name ? { name: info.name } : {}),
        endpointKind: endpoint.kind,
        endpointHost: endpoint.host,
      };
    } catch (error) {
      if (!isRetryableLanPeerError(error)) {
        return offlineProbe();
      }
    }
  }
  return offlineProbe();
}

/**
 * 会话前选出可达端点。返回的端点已通过 server-info 探测；会话 POST 只发一次，
 * 避免向两个地址重复请求导致重复同意窗。
 */
async function selectReachableEndpoint(
  target: PeerTarget,
): Promise<{ ok: true; endpoint: LanPeerEndpoint } | { ok: false; error: unknown }> {
  let lastError: unknown = new LanPeerRequestError("对端不可达", true);
  for (const endpoint of target.endpoints) {
    const timeoutMs = endpointProbeTimeoutMs(endpoint, target.endpoints.length);
    try {
      await fetchLanPeerInfo({
        ...endpointRequest(endpoint),
        token: target.token,
        ...(timeoutMs !== undefined ? { timeoutMs } : {}),
      });
      return { ok: true, endpoint };
    } catch (error) {
      lastError = error;
      if (!isRetryableLanPeerError(error)) {
        return { ok: false, error };
      }
    }
  }
  return { ok: false, error: lastError };
}

/**
 * 请求对端受控会话：先探测选出可达端点，再 POST /api/remote-desktop/session
 * （设备令牌）→ 双票据（信令 + 媒体）。成功后 main 用返回的 wsUrl 建立信令连接
 * （票据在 WS 升级时单次消费）；mediaUrl 是给 renderer 的媒体面连接地址（契约 E：
 * 主控 host 从 POST 响应拼出，经 rdStartSession 直达 renderer——被控 host 不知道
 * 自身对外可达地址）。
 */
export async function openRemoteDesktopPeerSession(
  credentials: ICredentialService,
  serverId: string,
  quality: RemoteDesktopQuality = DEFAULT_REMOTE_DESKTOP_QUALITY,
  codec: RemoteDesktopCodec = DEFAULT_REMOTE_DESKTOP_CODEC,
): Promise<
  | { ok: true; wsUrl: string; mediaUrl: string }
  | { ok: false; errorCode?: RemoteDesktopErrorCode; error: string }
> {
  const target = await readPeerTarget(credentials, serverId.trim());
  if (!target) {
    return { ok: false, errorCode: "unauthorized", error: "尚未与该设备配对" };
  }
  if (!target.token) {
    return { ok: false, errorCode: "unauthorized", error: "对端令牌缺失，请重新配对" };
  }
  const selection = await selectReachableEndpoint(target);
  if (!selection.ok) {
    if (selection.error instanceof LanPeerRequestError && selection.error.code === "unauthorized") {
      return {
        ok: false,
        errorCode: "unauthorized",
        error: selection.error.message,
      };
    }
    return {
      ok: false,
      errorCode: "internal",
      error: `连接对端失败：${describeError(selection.error)}`,
    };
  }
  const endpoint = selection.endpoint;
  // 端点协议决定整条链路：https 的 HTTP 请求 + wss 的信令/媒体 WebSocket。
  const base = buildLanHttpBase(endpoint.host, endpoint.port, endpoint.scheme, endpoint.path);
  const url = new URL(buildLanApiUrl(base, "/api/remote-desktop/session"));
  url.searchParams.set("token", target.token);
  // 画质档位（P1-3）：被控据此定 fps/maxWidth/初始码率；旧对端忽略该参数。
  url.searchParams.set("quality", quality);
  // 编码 codec 偏好（HEVC 选型 2026-10-13）：被控记 sessionCodec 直达 helper；
  // 旧对端忽略该参数（回 h264），解码串始终由码流自描述，偏好不一致安全。
  url.searchParams.set("codec", codec);
  let response: Response;
  try {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), SESSION_REQUEST_TIMEOUT_MS);
    try {
      response = await fetch(url.toString(), {
        method: "POST",
        signal: controller.signal,
      });
    } finally {
      clearTimeout(timer);
    }
  } catch (error) {
    return {
      ok: false,
      errorCode: "internal",
      error: `连接对端失败：${describeError(error)}`,
    };
  }
  if (!response.ok) {
    // 被控端以稳定错误码为主契约（docs/specs/lan-remote-desktop.md 降级与失败语义）。
    // 这是对端级结果，不再向另一个端点重发（避免重复同意窗/重复占用会话）。
    let code: RemoteDesktopErrorCode | undefined;
    try {
      const payload = (await response.json()) as { code?: unknown };
      if (typeof payload.code === "string") {
        code = payload.code as RemoteDesktopErrorCode;
      }
    } catch {
      code = undefined;
    }
    return {
      ok: false,
      ...(code ? { errorCode: code } : {}),
      error: `对端拒绝了会话请求（HTTP ${response.status}${code ? ` / ${code}` : ""}）`,
    };
  }
  let ticket: string | undefined;
  let mediaTicket: string | undefined;
  try {
    const payload = (await response.json()) as { ticket?: unknown; mediaTicket?: unknown };
    if (typeof payload.ticket === "string" && payload.ticket) {
      ticket = payload.ticket;
    }
    if (typeof payload.mediaTicket === "string" && payload.mediaTicket) {
      mediaTicket = payload.mediaTicket;
    }
  } catch {
    ticket = undefined;
    mediaTicket = undefined;
  }
  if (!ticket) {
    return { ok: false, errorCode: "internal", error: "对端响应缺少会话票据" };
  }
  if (!mediaTicket) {
    // 旧对端（未升级到双票据协议）：媒体面无法建立，按版本过旧收口而不是先连上再失败。
    // 失败分支额外读一次 server-info 取对端版本，把「对端不支持」变成可执行的诊断；
    // 诊断请求失败不覆盖主错误，仍保留稳定错误码 unsupported。
    let peerVersion = "";
    try {
      const info = await fetchLanPeerInfo({
        ...endpointRequest(endpoint),
        token: target.token,
      });
      peerVersion = info.version.trim();
    } catch {
      peerVersion = "";
    }
    return {
      ok: false,
      errorCode: "unsupported",
      error: peerVersion
        ? `对端缺少媒体票据（对端版本 ${peerVersion}；请升级到同一版本并重启对端）`
        : "对端缺少媒体票据（版本过旧）",
    };
  }
  // WS 升级与 POST 同样要出示设备令牌：服务端全局鉴权（http.ts isTokenProtectedPath 命中
  // /ws/*）与升级中间件 resolveLanClientId 都从 query 读令牌，只带 ticket 会被 401 拒绝。
  const wsUrl = new URL(buildLanApiUrl(base, "/ws/remote-desktop"));
  wsUrl.protocol = `${toWebSocketScheme(endpoint.scheme)}:`;
  wsUrl.searchParams.set("ticket", ticket);
  wsUrl.searchParams.set("token", target.token);
  // 契约 E（Task 10 消费）：mediaUrl 形态固定为 连接地址 + mediaTicket，
  // 由主控 host 从被控 POST 响应拼出，经 rdStartSession 返回值直达 renderer。
  // 票据即凭证：media 端点豁免全局令牌鉴权（服务端按 spec 以单次消费票据为唯一凭证），
  // 因此这里不带 token，renderer 拿 mediaUrl 即可直连（信令 wsUrl 仍需 token）。
  const mediaUrl = new URL(buildLanApiUrl(base, "/ws/remote-desktop-media"));
  mediaUrl.protocol = `${toWebSocketScheme(endpoint.scheme)}:`;
  mediaUrl.searchParams.set("mediaTicket", mediaTicket);
  return { ok: true, wsUrl: wsUrl.toString(), mediaUrl: mediaUrl.toString() };
}
