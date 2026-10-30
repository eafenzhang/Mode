/**
 * Mode Cloud 设备访问令牌的静态加密（docs/specs/cloud-personal-accounts.md，v2）。
 *
 * 设备 enroll 自报的本机 API 令牌以 AES-256-GCM 加密落库（cloud_devices.
 * access_token_cipher），只有同账号主控能经目录读回。密钥来自：
 *   1. env `MODE_CLOUD_DB_KEY`（任意非空字符串，SHA-256 派生 32B 密钥）；
 *   2. 缺省时在数据目录自动生成 `cloud-db.key`（0600，随机 32B）。
 * 密钥丢失 = 密文不可解密，目录对该设备降级（无 accessToken），设备重新 enroll 自愈。
 */
import {
  createCipheriv,
  createDecipheriv,
  createHash,
  randomBytes,
  randomUUID,
} from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";

const KEY_BYTES = 32;

export function loadDbKey(options: { envKey?: string; dbPath: string }): Buffer {
  const raw = options.envKey?.trim();
  if (raw) {
    return createHash("sha256").update(raw).digest();
  }
  const keyPath = join(dirname(options.dbPath), "cloud-db.key");
  try {
    const existing = readFileSync(keyPath);
    if (existing.length === KEY_BYTES) {
      return existing;
    }
  } catch {
    // 首次启动：生成新密钥。
  }
  const key = randomBytes(KEY_BYTES);
  mkdirSync(dirname(keyPath), { recursive: true });
  writeFileSync(keyPath, key, { mode: 0o600 });
  return key;
}

export function encryptSecret(key: Buffer, plaintext: string): string {
  const iv = randomBytes(12);
  const cipher = createCipheriv("aes-256-gcm", key, iv);
  const encrypted = Buffer.concat([cipher.update(plaintext, "utf8"), cipher.final()]);
  return [
    randomUUID().slice(0, 8), // 前缀仅用于在库中肉眼区分密文列
    iv.toString("base64"),
    cipher.getAuthTag().toString("base64"),
    encrypted.toString("base64"),
  ].join(".");
}

export function decryptSecret(key: Buffer, cipherText: string): string | null {
  const parts = cipherText.split(".");
  if (parts.length !== 4) {
    return null;
  }
  try {
    const iv = Buffer.from(parts[1] ?? "", "base64");
    const tag = Buffer.from(parts[2] ?? "", "base64");
    const data = Buffer.from(parts[3] ?? "", "base64");
    const decipher = createDecipheriv("aes-256-gcm", key, iv);
    decipher.setAuthTag(tag);
    return Buffer.concat([decipher.update(data), decipher.final()]).toString("utf8");
  } catch {
    // 密钥轮换/损坏：调用方按「令牌不可用」降级。
    return null;
  }
}
