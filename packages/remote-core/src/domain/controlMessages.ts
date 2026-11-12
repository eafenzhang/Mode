import type { DeviceDescriptor, DeviceRole } from "./descriptor.js";

export type TransportKind = "lan" | "p2p" | "turn" | "relay";

export interface RelayAssignment {
  endpoint: string;
  ticket: string;
  nodeId: string;
  epoch: number;
}

export interface HelloPayload {
  role: DeviceRole;
  deviceId: string;
  accessToken: string;
  protocols: number[];
  capabilities: string[];
  clientVersion?: string;
  addresses?: string[];
}

export interface HelloAckPayload {
  protocol: number;
  connectionSessionId: string;
  heartbeatIntervalMs: number;
  maxControlFrameBytes: number;
  maxRelayFrameBytes: number;
  capabilities: string[];
  serverVersion?: string;
}

export interface ConnectRequestPayload {
  hostDeviceId: string;
  preferredTransports: TransportKind[];
}

/** LAN 直连候选：ticket 由账号服务下发，双方在 Control 通道上收到同一值。 */
export interface DirectAssignment {
  ticket: string;
  /** Host 上报的 LAN `host:port`，客户端按顺序尝试。 */
  addresses: string[];
}

export interface IceServerPayload {
  urls: string[];
  /** coturn REST 临时凭证：`<过期时间戳>:<deviceId>`。 */
  username?: string;
  credential?: string;
}

export interface ConnectIncomingPayload {
  connectionId: string;
  client: DeviceDescriptor;
  authorization: "account";
  relay?: RelayAssignment;
  direct?: DirectAssignment;
  /** Client 在 connect.request 里声明了 p2p 优先；Host 据此推迟 Relay attach。 */
  p2pPreferred?: boolean;
  /** WebRTC 选路所需的 ICE 服务器（含 coturn 临时凭证）；未启用 WebRTC 时缺省。 */
  iceServers?: IceServerPayload[];
}

export interface ConnectAcceptedPayload {
  connectionId: string;
  host: DeviceDescriptor;
  relay?: RelayAssignment;
  direct?: DirectAssignment;
  iceServers?: IceServerPayload[];
}

export interface P2POfferPayload {
  connectionId: string;
  sdp: string;
}

export interface P2PAnswerPayload {
  connectionId: string;
  sdp: string;
}

export interface P2PCandidatePayload {
  connectionId: string;
  candidate: string;
  sdpMid?: string;
}

export interface P2PClosePayload {
  connectionId: string;
  reason?: string;
}

export interface TransportSelectedPayload {
  connectionId: string;
  transport: TransportKind;
}

export interface SecureHandshakePayload {
  connectionId: string;
  targetDeviceId: string;
  step: 1 | 2;
  data: string;
}

export interface RelayPayload {
  connectionId: string;
  targetDeviceId: string;
  counter: number;
  ciphertext: string;
}

export interface RelayAttachPayload {
  connectionId: string;
  deviceId: string;
  peerDeviceId: string;
  ticket: string;
}

export interface ErrorPayload {
  code: string;
  message: string;
  connectionId?: string;
  retryable?: boolean;
}

interface EnvelopeBase {
  v: 1;
  id: string;
  timestamp: number;
}

export type ControlFrame =
  | (EnvelopeBase & { type: "hello"; payload: HelloPayload })
  | (EnvelopeBase & { type: "hello.ack"; payload: HelloAckPayload })
  | (EnvelopeBase & { type: "connect.request"; payload: ConnectRequestPayload })
  | (EnvelopeBase & { type: "connect.incoming"; payload: ConnectIncomingPayload })
  | (EnvelopeBase & { type: "connect.accepted"; payload: ConnectAcceptedPayload })
  | (EnvelopeBase & { type: "transport.selected"; payload: TransportSelectedPayload })
  | (EnvelopeBase & { type: "secure.handshake"; payload: SecureHandshakePayload })
  | (EnvelopeBase & { type: "relay"; payload: RelayPayload })
  | (EnvelopeBase & { type: "relay.attach"; payload: RelayAttachPayload })
  | (EnvelopeBase & { type: "relay.ready"; payload: { connectionId: string } })
  | (EnvelopeBase & { type: "relay.detach"; payload: { connectionId: string; reason?: string } })
  | (EnvelopeBase & { type: "ping"; payload: { nonce?: string } })
  | (EnvelopeBase & { type: "pong"; payload: { nonce?: string } })
  | (EnvelopeBase & { type: "p2p.offer"; payload: P2POfferPayload })
  | (EnvelopeBase & { type: "p2p.answer"; payload: P2PAnswerPayload })
  | (EnvelopeBase & { type: "p2p.candidate"; payload: P2PCandidatePayload })
  | (EnvelopeBase & { type: "p2p.close"; payload: P2PClosePayload })
  | (EnvelopeBase & { type: "error"; payload: ErrorPayload });

export const CONTROL_TYPES = new Set<ControlFrame["type"]>([
  "hello",
  "hello.ack",
  "connect.request",
  "connect.incoming",
  "connect.accepted",
  "transport.selected",
  "secure.handshake",
  "relay",
  "relay.attach",
  "relay.ready",
  "relay.detach",
  "ping",
  "pong",
  "p2p.offer",
  "p2p.answer",
  "p2p.candidate",
  "p2p.close",
  "error",
]);

export const TRANSPORTS = new Set<TransportKind>(["lan", "p2p", "turn", "relay"]);
