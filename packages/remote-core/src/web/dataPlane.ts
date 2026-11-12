import type { P2PAdapter } from "../app/p2p.js";
import type { RemoteSocket } from "../app/controlTypes.js";
import { openWebDirectSocket, readDirectAssignment } from "./directSocket.js";
import { openWebP2P, readIceServers, type WebP2PSession } from "./p2pSession.js";
import type { WebSocketLike } from "./transport.js";

interface DataPlaneOptions {
  adapter: P2PAdapter | null;
  payload: Record<string, unknown>;
  connectionId: string;
  p2pDisabled: boolean;
  p2pTimeoutMs?: number;
  createSocket?: (url: string) => Promise<WebSocketLike>;
  sendControl: (type: string, payload: Record<string, unknown>) => void;
  onSession: (session: WebP2PSession) => void;
}

export interface WebDataPlane {
  dataSocket?: RemoteSocket;
  transport: "p2p" | "lan" | "relay";
  p2pSession: WebP2PSession | null;
}

/**
 * 按 `p2p -> lan -> relay` 选路：先尝试 WebRTC DataChannel，未打通再试 Host 上报的
 * LAN 直连地址；两者都失败时返回 relay，由调用方打开 Relay WSS。
 */
export async function selectWebDataPlane(options: DataPlaneOptions): Promise<WebDataPlane> {
  // 显式 p2p:false 的语义是“只走 Relay”，LAN 直连同样跳过。
  if (options.p2pDisabled) return { transport: "relay", p2pSession: null };
  const p2p =
    options.adapter && !options.p2pDisabled && options.payload.p2pPreferred === true
      ? await openWebP2P({
          adapter: options.adapter,
          ...(options.p2pTimeoutMs !== undefined ? { timeoutMs: options.p2pTimeoutMs } : {}),
          connectionId: options.connectionId,
          iceServers: readIceServers(options.payload.iceServers),
          send: (type, framePayload) => options.sendControl(type, framePayload),
          // P2P 失败只影响选路，不能 reject 握手/Relay 等待器。
          onError: () => undefined,
          onSession: options.onSession,
        })
      : null;
  const p2pSocket = p2p?.socket;
  let directSocket: RemoteSocket | undefined;
  const direct = readDirectAssignment(options.payload.direct);
  if (!p2pSocket && direct) {
    directSocket = await openWebDirectSocket({
      direct,
      connectionId: options.connectionId,
      ...(options.createSocket ? { createSocket: options.createSocket } : {}),
    });
  }
  const dataSocket = p2pSocket ?? directSocket;
  return {
    ...(dataSocket ? { dataSocket } : {}),
    transport: p2pSocket ? "p2p" : directSocket ? "lan" : "relay",
    p2pSession: p2p?.session ?? null,
  };
}
