import type { P2PAdapter } from "../app/p2p.js";
import type { TransportKind } from "../domain/controlMessages.js";
import type { WebIdentity } from "./identity.js";
import type { WebSocketLike } from "./transport.js";

export interface WebControlLinkOptions {
  serverUrl: string;
  deviceId: string;
  accessToken: string;
  identity: WebIdentity;
  connectTimeoutMs?: number;
  /** 断线重连的首次退避；测试可调小。 */
  reconnectBaseMs?: number;
  /** 测试注入点；缺省使用 globalThis.WebSocket。 */
  createSocket?: (url: string) => Promise<WebSocketLike>;
  /** 浏览器 WebRTC：缺省在支持 RTCPeerConnection 时启用；传 false 强制走 Relay。 */
  p2p?: { adapter?: P2PAdapter; timeoutMs?: number } | false;
  /** 选路结果（p2p/lan/relay）；用于观测、UI 展示与测试。 */
  onTransport?: (transport: TransportKind) => void;
}
