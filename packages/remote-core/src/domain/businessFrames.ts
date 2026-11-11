import { decodeBase64Url } from "./base64url.js";
import { invalid } from "./errors.js";
import { MAX_SECURE_MESSAGE_BYTES, PROTOCOL_VERSION } from "./limits.js";
import {
  asRecord,
  exactKeys,
  readBoolean,
  readInt,
  readObject,
  readString,
} from "./validation.js";

export interface RpcRequestFrame {
  v: 1;
  type: "rpc.request";
  id: string;
  method: string;
  params: Record<string, unknown>;
}

export interface RpcResponseFrame {
  v: 1;
  type: "rpc.response";
  id: string;
  result: unknown;
}

export interface RpcErrorFrame {
  v: 1;
  type: "rpc.error";
  id: string;
  code: string;
  message: string;
  retryable: boolean;
}

export interface EventFrame {
  v: 1;
  type: "event";
  topic: string;
  data: unknown;
}

export interface StreamOpenFrame {
  v: 1;
  type: "stream.open";
  streamId: number;
  service: string;
  metadata: Record<string, unknown>;
}

export interface StreamDataFrame {
  v: 1;
  type: "stream.data";
  streamId: number;
  data: string;
}

export interface StreamCloseFrame {
  v: 1;
  type: "stream.close";
  streamId: number;
  reason?: string;
}

export interface StreamErrorFrame {
  v: 1;
  type: "stream.error";
  streamId: number;
  code: string;
  message: string;
}

export interface PingFrame {
  v: 1;
  type: "ping";
  nonce?: string;
}

export interface PongFrame {
  v: 1;
  type: "pong";
  nonce?: string;
}

export type BusinessFrame =
  | RpcRequestFrame
  | RpcResponseFrame
  | RpcErrorFrame
  | EventFrame
  | StreamOpenFrame
  | StreamDataFrame
  | StreamCloseFrame
  | StreamErrorFrame
  | PingFrame
  | PongFrame;

const BUSINESS_TYPES = new Set<BusinessFrame["type"]>([
  "rpc.request",
  "rpc.response",
  "rpc.error",
  "event",
  "stream.open",
  "stream.data",
  "stream.close",
  "stream.error",
  "ping",
  "pong",
]);

export function encodeBusinessFrame(frame: BusinessFrame): Uint8Array {
  const bytes = Buffer.from(JSON.stringify(frame), "utf8");
  if (bytes.length > MAX_SECURE_MESSAGE_BYTES) {
    invalid(`业务消息超过 ${MAX_SECURE_MESSAGE_BYTES} 字节上限`);
  }
  return bytes;
}

export function decodeBusinessFrame(bytes: Uint8Array): BusinessFrame {
  if (bytes.length > MAX_SECURE_MESSAGE_BYTES) invalid("业务消息超过上限");
  let parsed: unknown;
  try {
    parsed = JSON.parse(Buffer.from(bytes).toString("utf8"));
  } catch {
    invalid("业务消息不是合法 JSON");
  }
  const record = asRecord(parsed, "business");
  if (record.v !== PROTOCOL_VERSION) invalid("业务消息协议版本不匹配");
  const type = record.type;
  if (typeof type !== "string" || !BUSINESS_TYPES.has(type as BusinessFrame["type"])) {
    invalid("未知业务消息类型");
  }
  switch (type as BusinessFrame["type"]) {
    case "rpc.request": {
      exactKeys(record, ["v", "type", "id", "method", "params"], "rpc.request");
      return {
        v: 1,
        type: "rpc.request",
        id: readString(record, "id", "rpc.request", { max: 128 }),
        method: readString(record, "method", "rpc.request", { max: 256 }),
        params: readObject(record, "params", "rpc.request") ?? {},
      };
    }
    case "rpc.response": {
      exactKeys(record, ["v", "type", "id", "result"], "rpc.response");
      if (!("result" in record)) invalid("rpc.response.result 缺失");
      return {
        v: 1,
        type: "rpc.response",
        id: readString(record, "id", "rpc.response", { max: 128 }),
        result: record.result,
      };
    }
    case "rpc.error": {
      exactKeys(record, ["v", "type", "id", "code", "message", "retryable"], "rpc.error");
      return {
        v: 1,
        type: "rpc.error",
        id: readString(record, "id", "rpc.error", { max: 128 }),
        code: readString(record, "code", "rpc.error", { max: 64 }),
        message: readString(record, "message", "rpc.error", { max: 512 }),
        retryable: readBoolean(record, "retryable", "rpc.error", true),
      };
    }
    case "event": {
      exactKeys(record, ["v", "type", "topic", "data"], "event");
      return {
        v: 1,
        type: "event",
        topic: readString(record, "topic", "event", { max: 256 }),
        data: record.data,
      };
    }
    case "stream.open": {
      exactKeys(record, ["v", "type", "streamId", "service", "metadata"], "stream.open");
      return {
        v: 1,
        type: "stream.open",
        streamId: readInt(record, "streamId", "stream.open", { min: 1 }),
        service: readString(record, "service", "stream.open", { max: 256 }),
        metadata: readObject(record, "metadata", "stream.open") ?? {},
      };
    }
    case "stream.data": {
      exactKeys(record, ["v", "type", "streamId", "data"], "stream.data");
      const data = readString(record, "data", "stream.data", { max: MAX_SECURE_MESSAGE_BYTES });
      decodeBase64Url(data, "stream.data.data");
      return {
        v: 1,
        type: "stream.data",
        streamId: readInt(record, "streamId", "stream.data", { min: 1 }),
        data,
      };
    }
    case "stream.close": {
      exactKeys(record, ["v", "type", "streamId", "reason"], "stream.close");
      const frame: StreamCloseFrame = {
        v: 1,
        type: "stream.close",
        streamId: readInt(record, "streamId", "stream.close", { min: 1 }),
      };
      if (record.reason !== undefined) {
        frame.reason = readString(record, "reason", "stream.close", { max: 256 });
      }
      return frame;
    }
    case "stream.error": {
      exactKeys(record, ["v", "type", "streamId", "code", "message"], "stream.error");
      return {
        v: 1,
        type: "stream.error",
        streamId: readInt(record, "streamId", "stream.error", { min: 1 }),
        code: readString(record, "code", "stream.error", { max: 64 }),
        message: readString(record, "message", "stream.error", { max: 512 }),
      };
    }
    case "ping":
    case "pong": {
      exactKeys(record, ["v", "type", "nonce"], type);
      const frame: PingFrame | PongFrame = { v: 1, type: type as "ping" | "pong" };
      if (record.nonce !== undefined) {
        frame.nonce = readString(record, "nonce", type, { max: 64 });
      }
      return frame;
    }
  }
}
