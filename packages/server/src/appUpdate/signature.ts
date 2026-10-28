/**
 * fpk 在线升级的可选发布签名（docs/specs/fnos-feature-parity-and-update.md §3.6）。
 *
 * 只签 `sha256.txt`：文件里已经列出 `.fpk` 的哈希，验签通过即等于确认「这份哈希清单
 * 由持有私钥的发布者签过」。签名算法固定 Ed25519（Node 内置，无第三方依赖）：
 * - 公钥可放在 `$TRIM_PKGETC/update-public-key`：PEM，或裸 32 字节的 base64；
 * - 签名为 64 字节 Ed25519 的 base64，放在同一 release 的 `sha256.txt.sig`。
 * 未配置公钥时完全跳过（保持现有 TLS + 哈希信任模型），不假装做过签名校验。
 */
import { createPublicKey, verify, type KeyObject } from "node:crypto";

/** Ed25519 SubjectPublicKeyInfo 的固定 DER 前缀（后接 32 字节裸公钥）。 */
const ED25519_SPKI_PREFIX = Buffer.from("302a300506032b6570032100", "hex");

export function normalizeUpdatePublicKey(raw: string | undefined): KeyObject | null {
  const value = raw?.trim() ?? "";
  if (!value) {
    return null;
  }
  try {
    if (value.includes("BEGIN PUBLIC KEY")) {
      return createPublicKey({ key: value, format: "pem" });
    }
    const rawBytes = Buffer.from(value, "base64");
    if (rawBytes.length !== 32) {
      return null;
    }
    return createPublicKey({
      key: Buffer.concat([ED25519_SPKI_PREFIX, rawBytes]),
      format: "der",
      type: "spki",
    });
  } catch {
    return null;
  }
}

export function verifyChecksumsSignature(params: {
  checksums: string;
  signatureBase64: string;
  publicKey: KeyObject;
}): boolean {
  try {
    const signature = Buffer.from(params.signatureBase64.trim(), "base64");
    if (signature.length !== 64) {
      return false;
    }
    return verify(null, Buffer.from(params.checksums, "utf8"), params.publicKey, signature);
  } catch {
    return false;
  }
}
