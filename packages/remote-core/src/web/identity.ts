import { x25519 } from "@noble/curves/ed25519";

import { decodeBase64Url, encodeBase64Url } from "./base64url.js";

export const WEB_IDENTITY_KEY_BYTES = 32;

export interface WebIdentity {
  readonly privateKey: Uint8Array;
  readonly publicKey: Uint8Array;
  /** 32 字节 X25519 公钥的规范无 padding base64url。 */
  readonly identityKey: string;
}

function fromPrivateKey(privateKey: Uint8Array): WebIdentity {
  if (privateKey.length !== WEB_IDENTITY_KEY_BYTES) {
    throw new Error("X25519 私钥必须为 32 字节");
  }
  const publicKey = x25519.getPublicKey(privateKey);
  if (publicKey.length !== WEB_IDENTITY_KEY_BYTES) {
    throw new Error("X25519 公钥长度非法");
  }
  return { privateKey, publicKey, identityKey: encodeBase64Url(publicKey) };
}

export function generateWebIdentity(): WebIdentity {
  return fromPrivateKey(x25519.utils.randomPrivateKey());
}

/** 浏览器持久化格式：原始 32 字节私钥的 base64url（由 IndexedDB/安全存储保管）。 */
export function importWebIdentity(privateKeyBase64: string): WebIdentity {
  return fromPrivateKey(decodeBase64Url(privateKeyBase64, "webIdentity.privateKey"));
}

export function exportWebIdentity(identity: WebIdentity): string {
  return encodeBase64Url(identity.privateKey);
}
