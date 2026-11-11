import {
  createPrivateKey,
  createPublicKey,
  diffieHellman,
  generateKeyPairSync,
  type KeyObject,
} from "node:crypto";

import { decodeBase64Url, encodeBase64Url } from "../domain/base64url.js";
import { isValidIdentityKey } from "../domain/descriptor.js";
import { RemoteError } from "../domain/errors.js";
import { IDENTITY_KEY_BYTES } from "../domain/limits.js";

export interface IdentityKeyPair {
  readonly privateKey: KeyObject;
  readonly publicKey: KeyObject;
  /** 32 字节 X25519 公钥的规范无 padding base64url。 */
  readonly identityKey: string;
}

export function generateIdentity(): IdentityKeyPair {
  const { privateKey, publicKey } = generateKeyPairSync("x25519");
  return fromKeyObjects(privateKey, publicKey);
}

/** 从 PKCS#8 DER（base64）恢复设备私钥；公钥由私钥派生，永不单独落盘。 */
export function importIdentityFromPkcs8(pkcs8Base64: string): IdentityKeyPair {
  const privateKey = createPrivateKey({
    key: Buffer.from(pkcs8Base64, "base64"),
    format: "der",
    type: "pkcs8",
  });
  return fromKeyObjects(privateKey, createPublicKey(privateKey));
}

export function exportIdentityPrivateKey(identity: IdentityKeyPair): string {
  return identity.privateKey.export({ format: "der", type: "pkcs8" }).toString("base64");
}

export function identityPublicKeyRaw(identity: IdentityKeyPair): Uint8Array {
  return decodeBase64Url(identity.identityKey, "identityKey");
}

export function publicKeyObject(identityKey: string): KeyObject {
  if (!isValidIdentityKey(identityKey)) {
    throw new RemoteError("invalid_identity_key", "X25519 公钥格式非法");
  }
  return createPublicKey({
    key: { kty: "OKP", crv: "X25519", x: identityKey },
    format: "jwk",
  });
}

/** X25519 DH；低阶点导致全零共享密钥时 fail closed。 */
export function x25519(privateKey: KeyObject, peerPublicKey: Uint8Array): Buffer {
  if (peerPublicKey.length !== IDENTITY_KEY_BYTES) {
    throw new RemoteError("invalid_identity_key", "X25519 公钥必须为 32 字节");
  }
  const shared = diffieHellman({
    privateKey,
    publicKey: publicKeyObject(encodeBase64Url(peerPublicKey)),
  });
  if (shared.length !== IDENTITY_KEY_BYTES || shared.every((byte) => byte === 0)) {
    throw new RemoteError("invalid_shared_secret", "X25519 共享密钥非法");
  }
  return shared;
}

function fromKeyObjects(privateKey: KeyObject, publicKey: KeyObject): IdentityKeyPair {
  const jwk = publicKey.export({ format: "jwk" }) as { x?: string };
  if (!jwk.x || jwk.x.length !== 43) {
    throw new RemoteError("invalid_identity_key", "无法导出 X25519 公钥");
  }
  const raw = decodeBase64Url(jwk.x, "identityKey");
  if (raw.length !== IDENTITY_KEY_BYTES) {
    throw new RemoteError("invalid_identity_key", "X25519 公钥长度非法");
  }
  return { privateKey, publicKey, identityKey: encodeBase64Url(raw) };
}
