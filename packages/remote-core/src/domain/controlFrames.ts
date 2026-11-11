import {
  parseConnectAccepted,
  parseConnectIncoming,
  parseConnectRequest,
  parseConnectionOnly,
  parseError,
  parseHello,
  parseHelloAck,
  parseRelay,
  parseRelayAttach,
  parseSecureHandshake,
  parseTransportSelected,
} from "./controlPayloads.js";
import { invalid } from "./errors.js";
import {
  MAX_CONTROL_FRAME_BYTES,
  MAX_RELAY_FRAME_BYTES,
  PROTOCOL_VERSION,
} from "./limits.js";
import { asRecord, exactKeys, readInt, readString } from "./validation.js";
import { CONTROL_TYPES, type ControlFrame } from "./controlMessages.js";

export function encodeControlFrame(frame: ControlFrame): Uint8Array {
  const bytes = Buffer.from(JSON.stringify(frame), "utf8");
  const limit = frame.type === "relay" ? MAX_RELAY_FRAME_BYTES : MAX_CONTROL_FRAME_BYTES;
  if (bytes.length > limit) invalid(`control frame 超过 ${limit} 字节`);
  return bytes;
}

export function decodeControlFrame(bytes: Uint8Array): ControlFrame {
  if (bytes.length > MAX_RELAY_FRAME_BYTES) invalid("control frame 超过上限");
  let parsed: unknown;
  try {
    parsed = JSON.parse(Buffer.from(bytes).toString("utf8"));
  } catch {
    invalid("control frame 不是合法 JSON");
  }
  const record = asRecord(parsed, "frame");
  exactKeys(record, ["v", "id", "type", "timestamp", "payload"], "frame");
  if (record.v !== PROTOCOL_VERSION) invalid("不支持的协议版本");
  const id = readString(record, "id", "frame", { max: 128 });
  const timestamp = readInt(record, "timestamp", "frame", { min: 1 });
  const type = record.type;
  if (typeof type !== "string" || !CONTROL_TYPES.has(type as ControlFrame["type"])) {
    invalid("未知 control frame 类型");
  }
  const payload = asRecord(record.payload, "frame.payload");
  const envelope = { v: PROTOCOL_VERSION, id, timestamp } as const;
  switch (type as ControlFrame["type"]) {
    case "hello":
      return { ...envelope, type: "hello", payload: parseHello(payload) };
    case "hello.ack":
      return { ...envelope, type: "hello.ack", payload: parseHelloAck(payload) };
    case "connect.request":
      return { ...envelope, type: "connect.request", payload: parseConnectRequest(payload) };
    case "connect.incoming":
      return { ...envelope, type: "connect.incoming", payload: parseConnectIncoming(payload) };
    case "connect.accepted":
      return { ...envelope, type: "connect.accepted", payload: parseConnectAccepted(payload) };
    case "transport.selected":
      return { ...envelope, type: "transport.selected", payload: parseTransportSelected(payload) };
    case "secure.handshake":
      return { ...envelope, type: "secure.handshake", payload: parseSecureHandshake(payload) };
    case "relay":
      return { ...envelope, type: "relay", payload: parseRelay(payload) };
    case "relay.attach":
      return { ...envelope, type: "relay.attach", payload: parseRelayAttach(payload) };
    case "relay.ready":
      return { ...envelope, type: "relay.ready", payload: parseConnectionOnly(payload) };
    case "relay.detach": {
      exactKeys(payload, ["connectionId", "reason"], "relay.detach");
      const connectionId = readString(payload, "connectionId", "relay.detach", { max: 64 });
      const reason =
        payload.reason === undefined
          ? undefined
          : readString(payload, "reason", "relay.detach", { max: 256 });
      return { ...envelope, type: "relay.detach", payload: { connectionId, reason } };
    }
    case "ping":
    case "pong": {
      exactKeys(payload, ["nonce"], type);
      const nonce =
        payload.nonce === undefined
          ? undefined
          : readString(payload, "nonce", type, { max: 64 });
      return { ...envelope, type: type as "ping" | "pong", payload: { nonce } };
    }
    case "error":
      return { ...envelope, type: "error", payload: parseError(payload) };
  }
}

export type {
  ConnectAcceptedPayload,
  ConnectIncomingPayload,
  ConnectRequestPayload,
  ControlFrame,
  DirectAssignment,
  ErrorPayload,
  HelloAckPayload,
  HelloPayload,
  RelayAssignment,
  RelayAttachPayload,
  RelayPayload,
  SecureHandshakePayload,
  TransportKind,
  TransportSelectedPayload,
} from "./controlMessages.js";
