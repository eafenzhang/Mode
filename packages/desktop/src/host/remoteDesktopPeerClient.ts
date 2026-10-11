/**
 * 主控方向的对端操作（host 侧）：对端能力位探测与受控会话请求。
 *
 * 为什么放在 host 而不在 main：对端长期令牌保存在凭据服务（`lan:peer:<serverId>:token`），
 * host 是本机 LAN 客户端凭据的既有持有者；main 只拿到「票据 + WS 地址」，令牌不出 host。
 */
import {
  buildLanPeerMetaKey,
  buildLanPeerTokenKey,
  lanPeerMetaSchema,
} from "@mode/shared";
import type { RemoteDesktopErrorCode } from "@mode/shared/remote-desktop";
import type { ICredentialService } from "@mode/services";
import { fetchLanPeerInfo } from "./lanRemoteAttach.js";

const PEER_REQUEST_TIMEOUT_MS = 10_000;

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

async function fetchWithTimeout(url: string, init: RequestInit): Promise<Response> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), PEER_REQUEST_TIMEOUT_MS);
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
 * 请求对端受控会话：POST /api/remote-desktop/session（设备令牌）→ 一次性票据。
 * 成功后 main 用返回的 wsUrl 建立信令连接（票据在 WS 升级时单次消费）。
 */
export async function openRemoteDesktopPeerSession(
  credentials: ICredentialService,
  serverId: string,
): Promise<{ ok: true; wsUrl: string } | { ok: false; errorCode?: RemoteDesktopErrorCode; error: string }> {
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
    response = await fetchWithTimeout(url.toString(), { method: "POST" });
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
  try {
    const payload = (await response.json()) as { ticket?: unknown };
    if (typeof payload.ticket === "string" && payload.ticket) {
      ticket = payload.ticket;
    }
  } catch {
    ticket = undefined;
  }
  if (!ticket) {
    return { ok: false, errorCode: "internal", error: "对端响应缺少会话票据" };
  }
  return { ok: true, wsUrl: `ws://${target.host}:${target.port}/ws/remote-desktop?ticket=${ticket}` };
}
