import {
  createCipheriv,
  createDecipheriv,
  createHash,
  createHmac,
  hkdfSync,
} from "node:crypto";

import { decodeBase64Url } from "../domain/base64url.js";
import { RemoteError } from "../domain/errors.js";
import { MAX_NOISE_MESSAGE_BYTES } from "../domain/limits.js";
import { generateIdentity, identityPublicKeyRaw, x25519, type IdentityKeyPair } from "./identity.js";

const PROTOCOL_NAME = Buffer.from("Noise_IK_25519_ChaChaPoly_SHA256", "utf8");
const HASH_BYTES = 32;
const KEY_BYTES = 32;
const TAG_BYTES = 16;
const EMPTY = Buffer.alloc(0);

export interface NoiseHandshakeOptions {
  initiator: boolean;
  localIdentity: IdentityKeyPair;
  /** Iniator：Responder static；Responder：自己的 static（pre-message 混合双方一致的 Key）。 */
  responderStaticKey: string;
  /** Responder 必须校验 msg1 中携带的 Initiator static；Initiator 传 null。 */
  expectedPeerStaticKey?: string | null;
  prologue: Uint8Array;
  /** 测试注入固定 ephemeral key；生产不传。 */
  ephemeral?: IdentityKeyPair;
}

export interface NoiseTransportPair {
  sendKey: Buffer;
  receiveKey: Buffer;
}

/**
 * Noise_IK_25519_ChaChaPoly_SHA256 的两消息握手。
 * 不自行发明握手状态机；阶段与 token 顺序严格按 Noise IK。
 */
export class NoiseHandshake {
  private readonly options: NoiseHandshakeOptions;
  private readonly localStatic: Uint8Array;
  private readonly responderStatic: Uint8Array;
  private h: Buffer;
  private ck: Buffer;
  private k: Buffer | null = null;
  private nonce = 0n;
  private ephemeral: IdentityKeyPair | null = null;
  private remoteEphemeral: Uint8Array | null = null;
  private peerStatic: Uint8Array | null = null;
  private step: 0 | 1 | 2 = 0;
  private complete = false;

  constructor(options: NoiseHandshakeOptions) {
    this.options = options;
    this.localStatic = identityPublicKeyRaw(options.localIdentity);
    this.responderStatic = decodeBase64Url(options.responderStaticKey, "responderStaticKey");
    this.h = sha256(PROTOCOL_NAME);
    this.ck = Buffer.from(this.h);
    this.mixHash(Buffer.from(options.prologue));
    this.mixHash(Buffer.from(this.responderStatic));
  }

  get isComplete(): boolean {
    return this.complete;
  }

  writeMessage(): Uint8Array {
    if (this.complete) throw new RemoteError("noise_state", "握手已完成");
    if (this.options.initiator) {
      if (this.step !== 0) throw new RemoteError("noise_state", "Initiator 只能写第一条消息");
      const ephemeral = this.options.ephemeral ?? generateEphemeral();
      this.ephemeral = ephemeral;
      const ePublic = identityPublicKeyRaw(ephemeral);
      this.mixHash(Buffer.from(ePublic));
      this.mixKey(x25519(ephemeral.privateKey, this.responderStatic));
      const encryptedStatic = this.encryptAndHash(Buffer.from(this.localStatic));
      this.mixKey(x25519(this.options.localIdentity.privateKey, this.responderStatic));
      this.step = 1;
      return Buffer.concat([Buffer.from(ePublic), encryptedStatic]);
    }
    if (this.step !== 1 || !this.remoteEphemeral || !this.peerStatic) {
      throw new RemoteError("noise_state", "Responder 必须先读取第一条消息");
    }
    const ephemeral = this.options.ephemeral ?? generateEphemeral();
    this.ephemeral = ephemeral;
    const ePublic = identityPublicKeyRaw(ephemeral);
    this.mixHash(Buffer.from(ePublic));
    this.mixKey(x25519(ephemeral.privateKey, this.remoteEphemeral));
    // IK msg2 的 se = DH(initiator static, responder ephemeral)。
    this.mixKey(x25519(ephemeral.privateKey, this.peerStatic));
    this.step = 2;
    this.complete = true;
    return Buffer.from(ePublic);
  }

  readMessage(message: Uint8Array): void {
    if (message.length > MAX_NOISE_MESSAGE_BYTES) {
      throw new RemoteError("noise_message_too_large", "Noise 握手消息超过上限");
    }
    if (this.options.initiator) {
      if (this.step !== 1 || this.complete) {
        throw new RemoteError("noise_state", "Initiator 只能读取第二条消息");
      }
      if (message.length !== HASH_BYTES) {
        throw new RemoteError("noise_message", "Responder 握手消息长度非法");
      }
      const re = Buffer.from(message);
      this.remoteEphemeral = re;
      this.mixHash(re);
      if (!this.ephemeral) throw new RemoteError("noise_state", "缺少 ephemeral key");
      this.mixKey(x25519(this.ephemeral.privateKey, re));
      this.mixKey(x25519(this.options.localIdentity.privateKey, re));
      this.step = 2;
      this.complete = true;
      return;
    }
    if (this.step !== 0) throw new RemoteError("noise_state", "Responder 只能读取第一条消息");
    if (message.length !== HASH_BYTES * 2 + TAG_BYTES) {
      throw new RemoteError("noise_message", "Initiator 握手消息长度非法");
    }
    const re = Buffer.from(message.subarray(0, HASH_BYTES));
    this.remoteEphemeral = re;
    this.mixHash(re);
    this.mixKey(x25519(this.options.localIdentity.privateKey, re));
    const peerStatic = this.decryptAndHash(
      Buffer.from(message.subarray(HASH_BYTES)),
    );
    if (peerStatic.length !== HASH_BYTES) {
      throw new RemoteError("noise_message", "Initiator static key 长度非法");
    }
    const expected = this.options.expectedPeerStaticKey;
    if (expected) {
      const expectedRaw = decodeBase64Url(expected, "expectedPeerStaticKey");
      if (!Buffer.from(expectedRaw).equals(peerStatic)) {
        throw new RemoteError("identity_mismatch", "Noise 对端 static key 与固定身份不一致");
      }
    }
    const peerStaticBytes = Buffer.from(peerStatic);
    this.peerStatic = peerStaticBytes;
    this.mixKey(x25519(this.options.localIdentity.privateKey, peerStaticBytes));
    this.step = 1;
  }

  split(): NoiseTransportPair {
    if (!this.complete) throw new RemoteError("noise_state", "握手未完成，不能 split");
    const okm = Buffer.from(
      hkdfSync("sha256", Buffer.alloc(HASH_BYTES), this.ck, EMPTY, KEY_BYTES * 2),
    );
    const k1 = okm.subarray(0, KEY_BYTES);
    const k2 = okm.subarray(KEY_BYTES, KEY_BYTES * 2);
    return this.options.initiator
      ? { sendKey: Buffer.from(k1), receiveKey: Buffer.from(k2) }
      : { sendKey: Buffer.from(k2), receiveKey: Buffer.from(k1) };
  }

  private mixHash(data: Buffer): void {
    this.h = sha256(Buffer.concat([this.h, data]));
  }

  private mixKey(inputKeyMaterial: Buffer): void {
    const okm = Buffer.from(hkdfSync("sha256", inputKeyMaterial, this.ck, EMPTY, KEY_BYTES * 2));
    this.ck = okm.subarray(0, KEY_BYTES);
    this.k = okm.subarray(KEY_BYTES, KEY_BYTES * 2);
    this.nonce = 0n;
  }

  private encryptAndHash(plaintext: Buffer): Buffer {
    if (!this.k) throw new RemoteError("noise_state", "缺少握手密钥");
    const ciphertext = encryptWithAd(this.k, this.nonce, this.h, plaintext);
    this.nonce += 1n;
    this.mixHash(ciphertext);
    return ciphertext;
  }

  private decryptAndHash(ciphertext: Buffer): Buffer {
    if (!this.k) throw new RemoteError("noise_state", "缺少握手密钥");
    try {
      const plaintext = decryptWithAd(this.k, this.nonce, this.h, ciphertext);
      this.nonce += 1n;
      this.mixHash(ciphertext);
      return plaintext;
    } catch (error) {
      throw new RemoteError("noise_auth_failed", "Noise 握手认证失败", { cause: error });
    }
  }
}

/** 已认证的 Noise transport CipherState；每方向独立 nonce，失败即终止。 */
export class NoiseTransport {
  private sendKey: Buffer;
  private receiveKey: Buffer;
  private sendNonce = 0n;
  private receiveNonce = 0n;
  private failed = false;

  constructor(pair: NoiseTransportPair) {
    this.sendKey = pair.sendKey;
    this.receiveKey = pair.receiveKey;
  }

  send(plaintext: Uint8Array): Buffer {
    this.assertUsable();
    if (plaintext.length + TAG_BYTES > MAX_NOISE_MESSAGE_BYTES) {
      throw new RemoteError("noise_message_too_large", "Noise transport 明文超过上限");
    }
    const ciphertext = encryptWithAd(
      this.sendKey,
      this.sendNonce,
      EMPTY,
      Buffer.from(plaintext),
    );
    this.sendNonce += 1n;
    return ciphertext;
  }

  receive(ciphertext: Uint8Array): Buffer {
    this.assertUsable();
    if (ciphertext.length > MAX_NOISE_MESSAGE_BYTES) {
      this.failed = true;
      throw new RemoteError("noise_message_too_large", "Noise transport 密文超过上限");
    }
    try {
      const plaintext = decryptWithAd(
        this.receiveKey,
        this.receiveNonce,
        EMPTY,
        Buffer.from(ciphertext),
      );
      this.receiveNonce += 1n;
      return plaintext;
    } catch (error) {
      this.failed = true;
      throw new RemoteError("noise_auth_failed", "Noise transport 认证失败", {
        cause: error,
      });
    }
  }

  private assertUsable(): void {
    if (this.failed) throw new RemoteError("noise_failed", "Noise 通道已因认证失败终止");
  }
}

function generateEphemeral(): IdentityKeyPair {
  return generateIdentity();
}

function sha256(...chunks: Buffer[]): Buffer {
  const hash = createHash("sha256");
  for (const chunk of chunks) hash.update(chunk);
  return hash.digest();
}

function encryptWithAd(key: Buffer, nonce: bigint, ad: Buffer, plaintext: Buffer): Buffer {
  const cipher = createCipheriv("chacha20-poly1305", key, nonceBytes(nonce), {
    authTagLength: TAG_BYTES,
  });
  if (ad.length > 0) cipher.setAAD(ad, { plaintextLength: plaintext.length });
  const encrypted = Buffer.concat([cipher.update(plaintext), cipher.final()]);
  return Buffer.concat([encrypted, cipher.getAuthTag()]);
}

function decryptWithAd(key: Buffer, nonce: bigint, ad: Buffer, ciphertext: Buffer): Buffer {
  if (ciphertext.length < TAG_BYTES) {
    throw new RemoteError("noise_message", "Noise 密文短于认证标签");
  }
  const encrypted = ciphertext.subarray(0, ciphertext.length - TAG_BYTES);
  const tag = ciphertext.subarray(ciphertext.length - TAG_BYTES);
  const decipher = createDecipheriv("chacha20-poly1305", key, nonceBytes(nonce), {
    authTagLength: TAG_BYTES,
  });
  decipher.setAuthTag(tag);
  if (ad.length > 0) decipher.setAAD(ad, { plaintextLength: encrypted.length });
  return Buffer.concat([decipher.update(encrypted), decipher.final()]);
}

function nonceBytes(counter: bigint): Buffer {
  const nonce = Buffer.alloc(12);
  nonce.writeBigUInt64LE(counter, 4);
  return nonce;
}

export function hmacSha256(key: Uint8Array, data: Uint8Array): Buffer {
  return createHmac("sha256", key).update(data).digest();
}
