import { decodeBase64Url } from "@mode/remote-core";
import type {
  RemoteV3RemoteDesktopSession,
  RemoteV3RemoteDesktopStartParams,
} from "@mode/shared";

import type { RemoteV3ClientRuntime } from "./client.js";
import type { RemoteDesktopTunnelProxy } from "./rdProxy.js";

/** 通过 v3 HTTP/WS 原语建立远程桌面，并把信令/媒体桥接到本机回环代理。 */
export async function startV3RemoteDesktopSession(
  client: RemoteV3ClientRuntime,
  proxy: RemoteDesktopTunnelProxy,
  params: RemoteV3RemoteDesktopStartParams,
): Promise<RemoteV3RemoteDesktopSession> {
  const query = new URLSearchParams();
  if (params.quality) query.set("quality", params.quality);
  if (params.codec) query.set("codec", params.codec);
  const suffix = query.size > 0 ? `?${query.toString()}` : "";
  const response = await client.httpRequest({
    hostDeviceId: params.hostDeviceId,
    method: "POST",
    path: `/api/remote-desktop/session${suffix}`,
    // 被控端同意窗最长 60s；会话创建请求必须覆盖它。
    timeoutMs: 75_000,
  });
  if (response.status !== 200) {
    let code = `HTTP ${response.status}`;
    try {
      const body = JSON.parse(
        Buffer.from(decodeBase64Url(response.bodyBase64, "body")).toString(),
      ) as { code?: unknown };
      if (typeof body.code === "string") code = body.code;
    } catch {
      // 错误体不可解析时保留 HTTP 状态。
    }
    throw new Error(`远程桌面会话被拒绝：${code}`);
  }
  let ticket: string | undefined;
  let mediaTicket: string | undefined;
  try {
    const body = JSON.parse(
      Buffer.from(decodeBase64Url(response.bodyBase64, "body")).toString(),
    ) as { ticket?: unknown; mediaTicket?: unknown };
    if (typeof body.ticket === "string") ticket = body.ticket;
    if (typeof body.mediaTicket === "string") mediaTicket = body.mediaTicket;
  } catch {
    ticket = undefined;
    mediaTicket = undefined;
  }
  if (!ticket || !mediaTicket) throw new Error("对端响应缺少信令/媒体票据");
  const signal = await client.openStream(params.hostDeviceId, "remote.ws", {
    path: `/ws/remote-desktop?ticket=${encodeURIComponent(ticket)}`,
  });
  const media = await client.openStream(params.hostDeviceId, "remote.ws", {
    path: `/ws/remote-desktop-media?mediaTicket=${encodeURIComponent(mediaTicket)}`,
  });
  const urls = await proxy.register(signal, media, {
    // renderer 收到 media-ticket 后会用新票据重连同一回环 URL；代理据此重开媒体隧道。
    reopenMedia: (ticket) =>
      client.openStream(params.hostDeviceId, "remote.ws", {
        path: `/ws/remote-desktop-media?mediaTicket=${encodeURIComponent(ticket)}`,
      }),
  });
  // 两条 stream 打开时连接已缓存，transport 即本次数据面实际选路；
  // 拿不到时按 relay 展示（最保守的归因，状态条主要用于排障）。
  return { ...urls, transport: client.getTransport(params.hostDeviceId) ?? "relay" };
}
