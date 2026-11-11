import { decodeBase64Url } from "./base64url.js";
import { parseDeviceDescriptor } from "./descriptor.js";
import { RemoteError, invalid } from "./errors.js";
import { MAX_CONTROL_FRAME_BYTES, MAX_RELAY_FRAME_BYTES, PROTOCOL_VERSION } from "./limits.js";
import {
  asRecord,
  exactKeys,
  readBoolean,
  readInt,
  readObject,
  readString,
  readStringArray,
} from "./validation.js";
import { TRANSPORTS } from "./controlMessages.js";
import type {
  ConnectAcceptedPayload,
  ConnectIncomingPayload,
  ConnectRequestPayload,
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

export function parseHello(payload: Record<string, unknown>): HelloPayload {
  exactKeys(
    payload,
    ["role", "deviceId", "accessToken", "protocols", "capabilities", "clientVersion", "addresses"],
    "hello",
  );
  const role = payload.role;
  if (role !== "host" && role !== "client") invalid("hello.role 非法");
  const protocols = readArrayOfInts(payload, "protocols", "hello", 1, 16);
  if (!protocols.includes(PROTOCOL_VERSION)) invalid("hello 不含当前协议版本");
  const result: HelloPayload = {
    role,
    deviceId: readString(payload, "deviceId", "hello", { max: 64 }),
    accessToken: readString(payload, "accessToken", "hello", { max: 8192 }),
    protocols,
    capabilities: readStringArray(payload, "capabilities", "hello", { min: 1, max: 32 }),
  };
  if (payload.clientVersion !== undefined) {
    result.clientVersion = readString(payload, "clientVersion", "hello", { max: 64 });
  }
  if (payload.addresses !== undefined) {
    result.addresses = readStringArray(payload, "addresses", "hello", { max: 8 });
  }
  return result;
}

export function parseHelloAck(payload: Record<string, unknown>): HelloAckPayload {
  exactKeys(
    payload,
    [
      "protocol",
      "connectionSessionId",
      "heartbeatIntervalMs",
      "maxControlFrameBytes",
      "maxRelayFrameBytes",
      "capabilities",
      "serverVersion",
    ],
    "hello.ack",
  );
  const result: HelloAckPayload = {
    protocol: readInt(payload, "protocol", "hello.ack", { min: 1, max: 1 }),
    connectionSessionId: readString(payload, "connectionSessionId", "hello.ack", { max: 64 }),
    heartbeatIntervalMs: readInt(payload, "heartbeatIntervalMs", "hello.ack", { min: 1 }),
    maxControlFrameBytes: readInt(payload, "maxControlFrameBytes", "hello.ack", {
      min: 1,
      max: MAX_CONTROL_FRAME_BYTES,
    }),
    maxRelayFrameBytes: readInt(payload, "maxRelayFrameBytes", "hello.ack", {
      min: 1,
      max: MAX_RELAY_FRAME_BYTES,
    }),
    capabilities: readStringArray(payload, "capabilities", "hello.ack", { max: 32 }),
  };
  if (payload.serverVersion !== undefined) {
    result.serverVersion = readString(payload, "serverVersion", "hello.ack", { max: 64 });
  }
  return result;
}

export function parseConnectRequest(payload: Record<string, unknown>): ConnectRequestPayload {
  exactKeys(payload, ["hostDeviceId", "preferredTransports"], "connect.request");
  return {
    hostDeviceId: readString(payload, "hostDeviceId", "connect.request", { max: 64 }),
    preferredTransports: readTransports(payload, "connect.request"),
  };
}

function parseDirectAssignment(
  payload: Record<string, unknown>,
  label: string,
): DirectAssignment {
  exactKeys(payload, ["ticket", "addresses"], label);
  return {
    ticket: readString(payload, "ticket", label, { max: 4096 }),
    addresses: readStringArray(payload, "addresses", label, { min: 1, max: 8 }),
  };
}

export function parseConnectIncoming(payload: Record<string, unknown>): ConnectIncomingPayload {
  exactKeys(
    payload,
    ["connectionId", "client", "authorization", "relay", "direct"],
    "connect.incoming",
  );
  if (payload.authorization !== "account") invalid("connect.incoming.authorization 非法");
  const result: ConnectIncomingPayload = {
    connectionId: readString(payload, "connectionId", "connect.incoming", { max: 64 }),
    client: parseDeviceDescriptor(payload.client, "connect.incoming.client"),
    authorization: "account",
  };
  const relay = readObject(payload, "relay", "connect.incoming", true);
  if (relay) result.relay = parseRelayAssignment(relay, "connect.incoming.relay");
  const direct = readObject(payload, "direct", "connect.incoming", true);
  if (direct) result.direct = parseDirectAssignment(direct, "connect.incoming.direct");
  return result;
}

export function parseConnectAccepted(payload: Record<string, unknown>): ConnectAcceptedPayload {
  exactKeys(payload, ["connectionId", "host", "relay", "direct"], "connect.accepted");
  const result: ConnectAcceptedPayload = {
    connectionId: readString(payload, "connectionId", "connect.accepted", { max: 64 }),
    host: parseDeviceDescriptor(payload.host, "connect.accepted.host"),
  };
  const relay = readObject(payload, "relay", "connect.accepted", true);
  if (relay) result.relay = parseRelayAssignment(relay, "connect.accepted.relay");
  const direct = readObject(payload, "direct", "connect.accepted", true);
  if (direct) result.direct = parseDirectAssignment(direct, "connect.accepted.direct");
  return result;
}

export function parseTransportSelected(payload: Record<string, unknown>): TransportSelectedPayload {
  exactKeys(payload, ["connectionId", "transport"], "transport.selected");
  const transport = payload.transport;
  if (typeof transport !== "string" || !TRANSPORTS.has(transport as TransportKind)) {
    invalid("transport.selected.transport 非法");
  }
  return {
    connectionId: readString(payload, "connectionId", "transport.selected", { max: 64 }),
    transport: transport as TransportKind,
  };
}

export function parseSecureHandshake(payload: Record<string, unknown>): SecureHandshakePayload {
  exactKeys(payload, ["connectionId", "targetDeviceId", "step", "data"], "secure.handshake");
  const step = payload.step;
  if (step !== 1 && step !== 2) invalid("secure.handshake.step 只能是 1 或 2");
  const data = readString(payload, "data", "secure.handshake", { max: 8192 });
  decodeBase64Url(data, "secure.handshake.data");
  return {
    connectionId: readString(payload, "connectionId", "secure.handshake", { max: 64 }),
    targetDeviceId: readString(payload, "targetDeviceId", "secure.handshake", { max: 64 }),
    step,
    data,
  };
}

export function parseRelay(payload: Record<string, unknown>): RelayPayload {
  exactKeys(payload, ["connectionId", "targetDeviceId", "counter", "ciphertext"], "relay");
  const ciphertext = readString(payload, "ciphertext", "relay", { max: MAX_RELAY_FRAME_BYTES });
  const bytes = decodeBase64Url(ciphertext, "relay.ciphertext");
  if (bytes.length > MAX_RELAY_FRAME_BYTES) invalid("relay.ciphertext 超过上限");
  return {
    connectionId: readString(payload, "connectionId", "relay", { max: 64 }),
    targetDeviceId: readString(payload, "targetDeviceId", "relay", { max: 64 }),
    counter: readInt(payload, "counter", "relay"),
    ciphertext,
  };
}

export function parseRelayAttach(payload: Record<string, unknown>): RelayAttachPayload {
  exactKeys(payload, ["connectionId", "deviceId", "peerDeviceId", "ticket"], "relay.attach");
  return {
    connectionId: readString(payload, "connectionId", "relay.attach", { max: 64 }),
    deviceId: readString(payload, "deviceId", "relay.attach", { max: 64 }),
    peerDeviceId: readString(payload, "peerDeviceId", "relay.attach", { max: 64 }),
    ticket: readString(payload, "ticket", "relay.attach", { max: 4096 }),
  };
}

export function parseConnectionOnly(payload: Record<string, unknown>): { connectionId: string } {
  exactKeys(payload, ["connectionId"], "relay.ready");
  return { connectionId: readString(payload, "connectionId", "relay.ready", { max: 64 }) };
}

export function parseError(payload: Record<string, unknown>): ErrorPayload {
  exactKeys(payload, ["code", "message", "connectionId", "retryable"], "error");
  const result: ErrorPayload = {
    code: readString(payload, "code", "error", { max: 64 }),
    message: readString(payload, "message", "error", { max: 512 }),
  };
  if (payload.connectionId !== undefined) {
    result.connectionId = readString(payload, "connectionId", "error", { max: 64 });
  }
  if (payload.retryable !== undefined) {
    result.retryable = readBoolean(payload, "retryable", "error", true);
  }
  return result;
}

export function parseRelayAssignment(
  record: Record<string, unknown>,
  label: string,
): RelayAssignment {
  exactKeys(record, ["endpoint", "ticket", "nodeId", "epoch"], label);
  return {
    endpoint: readString(record, "endpoint", label, { max: 2048 }),
    ticket: readString(record, "ticket", label, { max: 4096 }),
    nodeId: readString(record, "nodeId", label, { max: 128 }),
    epoch: readInt(record, "epoch", label, { min: 0 }),
  };
}

export function readArrayOfInts(
  record: Record<string, unknown>,
  key: string,
  label: string,
  min: number,
  max: number,
): number[] {
  const value = record[key];
  if (!Array.isArray(value) || value.length < min || value.length > max) {
    invalid(`${label}.${key} 必须是 ${min}..${max} 个整数的数组`);
  }
  const result = value.map((entry) => {
    if (typeof entry !== "number" || !Number.isSafeInteger(entry) || entry < 0) {
      throw new RemoteError("invalid_message", `${label}.${key} 含非法整数`);
    }
    return entry;
  });
  if (new Set(result).size !== result.length) invalid(`${label}.${key} 不允许重复值`);
  return result;
}

export function readTransports(record: Record<string, unknown>, label: string): TransportKind[] {
  const values = readStringArray(record, "preferredTransports", label, { min: 1, max: 4 });
  return values.map((value) => {
    if (!TRANSPORTS.has(value as TransportKind)) {
      invalid(`${label}.preferredTransports 含非法值 ${value}`);
    }
    return value as TransportKind;
  });
}
