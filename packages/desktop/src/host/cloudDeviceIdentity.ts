/**
 * Mode Cloud 设备身份（docs/specs/mode-cloud-account-directory.md）。
 *
 * 设备 ID 复用 LAN 安装级 serverId；中继 tunnelId 复用已有凭据；Ed25519 私钥
 * 只存在加密凭据里，公钥随注册请求上传。私钥不写日志、不进 renderer。
 */
import {
  createPrivateKey,
  createPublicKey,
  generateKeyPairSync,
  randomBytes,
  randomUUID,
  sign,
} from "node:crypto";

import {
  CLOUD_DEVICE_PRIVATE_KEY_CREDENTIAL_KEY,
  LAN_ACCESS_RELAY_TUNNEL_ID_CREDENTIAL_KEY,
  LAN_ACCESS_SERVER_ID_CREDENTIAL_KEY,
} from "@mode/shared";
import type { ICredentialService } from "@mode/services";

export interface CloudDeviceIdentity {
  deviceId: string;
  relayTunnelId: string;
  privateKeyPem: string;
  publicKeyPem: string;
}

/** 递归按键排序，保证签名 payload 在不同进程中稳定。 */
function canonicalize(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map(canonicalize);
  }
  if (value && typeof value === "object") {
    const record = value as Record<string, unknown>;
    const result: Record<string, unknown> = {};
    for (const key of Object.keys(record).sort()) {
      result[key] = canonicalize(record[key]);
    }
    return result;
  }
  return value;
}

export function canonicalJson(value: unknown): string {
  return JSON.stringify(canonicalize(value));
}

export function signCloudEnrollment(privateKeyPem: string, payload: string): string {
  return sign(null, Buffer.from(payload, "utf8"), createPrivateKey(privateKeyPem)).toString(
    "base64",
  );
}

/**
 * 读取/生成设备身份。中继 tunnelId 缺失时本地生成并持久化：云端登录/注册需要它，
 * 隧道可以稍后（拿到 relay token 后）再启动；LAN 监听启动时复用同一个 key。
 */
export async function loadOrCreateCloudDeviceIdentity(
  credentials: ICredentialService,
): Promise<CloudDeviceIdentity> {
  let relayTunnelId = (
    await credentials.load(LAN_ACCESS_RELAY_TUNNEL_ID_CREDENTIAL_KEY)
  )?.trim() ?? "";
  if (!relayTunnelId) {
    relayTunnelId = randomBytes(24).toString("base64url");
    await credentials.save(LAN_ACCESS_RELAY_TUNNEL_ID_CREDENTIAL_KEY, relayTunnelId);
  }

  let deviceId = (await credentials.load(LAN_ACCESS_SERVER_ID_CREDENTIAL_KEY))?.trim() ?? "";
  if (!deviceId) {
    deviceId = randomUUID();
    await credentials.save(LAN_ACCESS_SERVER_ID_CREDENTIAL_KEY, deviceId);
  }

  let privateKeyPem = (await credentials.load(CLOUD_DEVICE_PRIVATE_KEY_CREDENTIAL_KEY))?.trim();
  if (!privateKeyPem) {
    const { privateKey } = generateKeyPairSync("ed25519");
    privateKeyPem = privateKey.export({ type: "pkcs8", format: "pem" }).toString();
    await credentials.save(CLOUD_DEVICE_PRIVATE_KEY_CREDENTIAL_KEY, privateKeyPem);
  }

  const publicKeyPem = createPublicKey(createPrivateKey(privateKeyPem))
    .export({ type: "spki", format: "pem" })
    .toString();

  return { deviceId, relayTunnelId, privateKeyPem, publicKeyPem };
}

/** 生成高熵 nonce；与 issuedAt 一起进入签名 payload，降低重放风险。 */
export function createEnrollmentNonce(): string {
  return randomBytes(16).toString("base64url");
}
