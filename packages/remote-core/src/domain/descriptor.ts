import { createHash } from "node:crypto";

import { decodeBase64Url, encodeBase64Url } from "./base64url.js";
import { invalid } from "./errors.js";
import {
  DEVICE_NAME_MAX_BYTES,
  IDENTITY_KEY_BYTES,
  PLATFORM_MAX_BYTES,
  VERSION_MAX_BYTES,
} from "./limits.js";
import { asRecord, exactKeys, readString } from "./validation.js";

export type DeviceRole = "host" | "client";

export interface DeviceDescriptor {
  deviceId: string;
  name: string;
  role: DeviceRole;
  platform: string;
  identityKey: string;
  clientVersion?: string;
  hostVersion?: string;
}

const UUID_PATTERN =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
const ULID_PATTERN = /^[0-9A-HJKMNP-TV-Z]{26}$/;
const PLATFORM_PATTERN = /^[a-z0-9][a-z0-9._-]*$/;
const VERSION_PATTERN = /^[0-9A-Za-z][0-9A-Za-z.+-]*$/;

export function isDeviceId(value: unknown): value is string {
  return typeof value === "string" && (UUID_PATTERN.test(value) || ULID_PATTERN.test(value));
}

export function isValidIdentityKey(value: unknown): value is string {
  if (typeof value !== "string" || value.length !== 43) return false;
  try {
    return decodeBase64Url(value, "identityKey").length === IDENTITY_KEY_BYTES;
  } catch {
    return false;
  }
}

export function parseDeviceDescriptor(value: unknown, label = "device"): DeviceDescriptor {
  const record = asRecord(value, label);
  exactKeys(
    record,
    ["deviceId", "name", "role", "platform", "identityKey", "clientVersion", "hostVersion"],
    label,
  );
  const deviceId = readString(record, "deviceId", label, { max: 64 });
  if (!isDeviceId(deviceId)) invalid(`${label}.deviceId 必须是 UUID 或 ULID`);
  const name = readString(record, "name", label, { max: DEVICE_NAME_MAX_BYTES });
  if (name.trim().length === 0) invalid(`${label}.name 不能为空白`);
  const roleValue = record.role;
  if (roleValue !== "host" && roleValue !== "client") {
    invalid(`${label}.role 只能是 host 或 client`);
  }
  const platform = readString(record, "platform", label, {
    max: PLATFORM_MAX_BYTES,
    pattern: PLATFORM_PATTERN,
  });
  const identityKey = readString(record, "identityKey", label, { max: 43 });
  if (!isValidIdentityKey(identityKey)) {
    invalid(`${label}.identityKey 必须是 32 字节 X25519 公钥的规范 base64url`);
  }
  const descriptor: DeviceDescriptor = {
    deviceId,
    name,
    role: roleValue,
    platform,
    identityKey,
  };
  if (record.clientVersion !== undefined) {
    descriptor.clientVersion = readString(record, "clientVersion", label, {
      max: VERSION_MAX_BYTES,
      pattern: VERSION_PATTERN,
    });
  }
  if (record.hostVersion !== undefined) {
    if (roleValue !== "host") invalid(`${label}.hostVersion 只允许 Host 携带`);
    descriptor.hostVersion = readString(record, "hostVersion", label, {
      max: VERSION_MAX_BYTES,
      pattern: VERSION_PATTERN,
    });
  }
  return descriptor;
}

export function encodeIdentityKey(raw: Uint8Array): string {
  if (raw.length !== IDENTITY_KEY_BYTES) invalid("X25519 公钥必须为 32 字节");
  return encodeBase64Url(raw);
}

/** 公钥指纹统一为 lowercaseHex(SHA-256(raw))，用于 UI 展示与日志关联。 */
export function identityFingerprint(identityKey: string): string {
  const raw = decodeBase64Url(identityKey, "identityKey");
  if (raw.length !== IDENTITY_KEY_BYTES) invalid("X25519 公钥必须为 32 字节");
  return createHash("sha256").update(raw).digest("hex");
}
