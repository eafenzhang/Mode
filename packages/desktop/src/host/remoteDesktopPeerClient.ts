/**
 * 主控方向的对端操作（host 侧）：对端能力位探测与受控会话请求。
 *
 * 为什么放在 host 而不在 main：对端长期令牌保存在凭据服务（`lan:peer:<serverId>:token`），
 * host 是本机 LAN 客户端凭据的既有持有者；令牌只随本模块的出站请求交给对端
 * （POST 与 WS 升级都用 `?token=` query，与服务端 `readPresentedLiteToken` 的读取口同形），
 * 不写日志、不进 renderer。
 */
import { buildLanPeerMetaKey, buildLanPeerTokenKey, lanPeerMetaSchema } from "@mode/shared";
import type { RemoteDesktopErrorCode } from "@mode/shared/remote-desktop";
import type { ICredentialService } from "@mode/services";
import { fetchLanPeerInfo } from "./lanRemoteAttach.js";

/**
 * 会话 POST 阻塞在被控同意流（60s 同意窗 + 75s 兜底，remoteDesktopServer.ts），
 * 主控预算必须覆盖整个等待期，否则用户没来得及点「允许」就被归因 internal（spec:
 * lan-remote-desktop 事件顺序 ② 主控侧等待预算 90s）。能力位探测不走本函数，
 * 由 fetchLanPeerInfo 自带短超时。
 */
const SESSION_REQUEST_TIMEOUT_MS = 90_000;

export interface RemoteDesktopPeerInfo {
  online: boolean;
  remoteDesktop: boolean;
  name?: string;
}

interface PeerTarget {
  host: string;
  port: number;
  token?: string;
}

async function readPeerTarget(
  credentials: ICredentialService,
  serverId: string,
): Promise<PeerTarget | null> {
  const rawMeta = await credentials.load(buildLanPeerMetaKey(serverId));
  if (!rawMeta) {
    return null;
  }
  let meta: unknown;
  try {
    meta = JSON.parse(rawMeta);
  } catch {
    return null;
  }
  const parsed = lanPeerMetaSchema.safeParse(meta);
  if (!parsed.success) {
    return null;
  }
  const token = (await credentials.load(buildLanPeerTokenKey(serverId)))?.trim();
  return {
    host: parsed.data.host,
    port: parsed.data.port,
    ...(token ? { token } : {}),
  };
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

/** 能力位探测：任何失败（未配对/离线/旧版）都归为 online:false，不抛给调用方。 */
export async function probeRemoteDesktopPeer(
  credentials: ICredentialService,
  serverId: string,
): Promise<RemoteDesktopPeerInfo> {
  const target = await readPeerTarget(credentials, serverId);
  if (!target) {
    return { online: false, remoteDesktop: false };
  }
  try {
    const info = await fetchLanPeerInfo({
      host: target.host,
      port: target.port,
      ...(target.token ? { token: target.token } : {}),
    });
    return {
      online: true,
      // 缺键 = 旧对端不支持（docs/specs/lan-remote-desktop.md 产品规则 5）。
      remoteDesktop: info.capabilities?.remoteDesktop === true,
      ...(info.name ? { name: info.name } : {}),
    };
  } catch {
    return { online: false, remoteDesktop: false };
  }
}

/**
 * 请求对端受控会话：POST /api/remote-desktop/session（设备令牌）→ 双票据（信令 + 媒体）。
 * 成功后 main 用返回的 wsUrl 建立信令连接（票据在 WS 升级时单次消费）；
 * mediaUrl 是给 renderer 的媒体面连接地址（契约 E：主控 host 从 POST 响应拼出，
 * 经 rdStartSession 直达 renderer——被控 host 不知道自身对外可达地址）。
 */
export async function openRemoteDesktopPeerSession(
  credentials: ICredentialService,
  serverId: string,
): Promise<
  | { ok: true; wsUrl: string; mediaUrl: string }
  | { ok: false; errorCode?: RemoteDesktopErrorCode; error: string }
> {
  const target = await readPeerTarget(credentials, serverId);
  if (!target) {
    return { ok: false, errorCode: "unauthorized", error: "尚未与该设备配对" };
  }
  if (!target.token) {
    return { ok: false, errorCode: "unauthorized", error: "对端令牌缺失，请重新配对" };
  }
  const base = `http://${target.host}:${target.port}`;
  const url = new URL("/api/remote-desktop/session", base);
  url.searchParams.set("token", target.token);
  let response: Response;
  try {
    response = await fetchWithTimeout(
      url.toString(),
      { method: "POST" },
      SESSION_REQUEST_TIMEOUT_MS,
    );
  } catch (error) {
    return {
      ok: false,
      errorCode: "internal",
      error: `连接对端失败：${error instanceof Error ? error.message : String(error)}`,
    };
  }
  if (!response.ok) {
    // 被控端以稳定错误码为主契约（docs/specs/lan-remote-desktop.md 降级与失败语义）。
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
    return {
      ok: false,
      errorCode: "unsupported",
      error: "对端缺少媒体票据（版本过旧）",
    };
  }
  // WS 升级与 POST 同样要出示设备令牌：服务端全局鉴权（http.ts isTokenProtectedPath 命中
  // /ws/*）与升级中间件的 resolveLanClientId 都从 query 读令牌，只带 ticket 会被 401 拒绝。
  const wsUrl = new URL(`/ws/remote-desktop`, base);
  wsUrl.protocol = "ws:";
  wsUrl.searchParams.set("ticket", ticket);
  wsUrl.searchParams.set("token", target.token);
  // 契约 E（Task 10 消费）：mediaUrl 形态固定为 连接地址 + mediaTicket，
  // 由主控 host 从被控 POST 响应拼出，经 rdStartSession 返回值直达 renderer。
  // 票据即凭证：media 端点豁免全局令牌鉴权（服务端按 spec 以单次消费票据为唯一凭证），
  // 因此这里不带 token，renderer 拿 mediaUrl 即可直连（信令 wsUrl 仍需 token）。
  const mediaUrl = new URL(`/ws/remote-desktop-media`, base);
  mediaUrl.protocol = "ws:";
  mediaUrl.searchParams.set("mediaTicket", mediaTicket);
  return { ok: true, wsUrl: wsUrl.toString(), mediaUrl: mediaUrl.toString() };
}
