/**
 * 云端 Bot token 的静态加密（AES-256-GCM）。
 * 密钥由 accountPepper 派生；token 只在启动 Bot 时解密到内存，不写日志。
 */
import { createCipheriv, createDecipheriv, createHash, randomBytes } from "node:crypto";

export interface EncryptedSecret {
  ciphertext: string;
  iv: string;
  tag: string;
}

function deriveKey(pepper: string): Buffer {
  return createHash("sha256").update(`cloud-bot:${pepper}`).digest();
}

export function encryptSecret(pepper: string, plaintext: string): EncryptedSecret {
  const iv = randomBytes(12);
  const cipher = createCipheriv("aes-256-gcm", deriveKey(pepper), iv);
  const ciphertext = Buffer.concat([cipher.update(plaintext, "utf8"), cipher.final()]);
  return {
    ciphertext: ciphertext.toString("base64"),
    iv: iv.toString("base64"),
    tag: cipher.getAuthTag().toString("base64"),
  };
}

export function decryptSecret(pepper: string, secret: EncryptedSecret): string {
  const decipher = createDecipheriv(
    "aes-256-gcm",
    deriveKey(pepper),
    Buffer.from(secret.iv, "base64"),
  );
  decipher.setAuthTag(Buffer.from(secret.tag, "base64"));
  return Buffer.concat([
    decipher.update(Buffer.from(secret.ciphertext, "base64")),
    decipher.final(),
  ]).toString("utf8");
}
