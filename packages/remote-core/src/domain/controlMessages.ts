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

export interface ConnectIncomingPayload {
  connectionId: string;
  client: DeviceDescriptor;
  authorization: "account";
  relay?: RelayAssignment;
  direct?: DirectAssignment;
}

export interface ConnectAcceptedPayload {
  connectionId: string;
  host: DeviceDescriptor;
  relay?: RelayAssignment;
  direct?: DirectAssignment;
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
  "error",
]);

export const TRANSPORTS = new Set<TransportKind>(["lan", "p2p", "turn", "relay"]);
