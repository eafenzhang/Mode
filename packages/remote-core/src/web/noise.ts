import { chacha20poly1305 } from "@noble/ciphers/chacha";
import { x25519 } from "@noble/curves/ed25519";
import { hkdf } from "@noble/hashes/hkdf";
import { sha256 } from "@noble/hashes/sha2";

import { decodeBase64Url } from "./base64url.js";
import { RemoteError } from "../domain/errors.js";
import { MAX_NOISE_MESSAGE_BYTES } from "../domain/limits.js";
import { generateWebIdentity, type WebIdentity } from "./identity.js";

const PROTOCOL_NAME = new TextEncoder().encode("Noise_IK_25519_ChaChaPoly_SHA256");
const HASH_BYTES = 32;
const KEY_BYTES = 32;
const TAG_BYTES = 16;
const EMPTY = new Uint8Array(0);

export interface WebNoiseHandshakeOptions {
  initiator: boolean;
  localIdentity: WebIdentity;
  /** Initiator：Responder static；Responder：自己的 static。 */
  responderStaticKey: string;
  /** Responder 必须校验 msg1 中的 Initiator static；Initiator 传 null。 */
  expectedPeerStaticKey?: string | null;
  prologue: Uint8Array;
  /** 测试注入固定 ephemeral key；生产不传。 */
  ephemeral?: WebIdentity;
}

export interface WebNoiseTransportPair {
  sendKey: Uint8Array;
  receiveKey: Uint8Array;
}

/** 浏览器版 Noise_IK_25519_ChaChaPoly_SHA256；线格式与 Node 实现完全一致。 */
export class WebNoiseHandshake {
  private readonly options: WebNoiseHandshakeOptions;
  private readonly localStatic: Uint8Array;
  private readonly responderStatic: Uint8Array;
  private h: Uint8Array;
  private ck: Uint8Array;
  private k: Uint8Array | null = null;
  private nonce = 0n;
  private ephemeral: WebIdentity | null = null;
  private remoteEphemeral: Uint8Array | null = null;
  private peerStatic: Uint8Array | null = null;
  private step: 0 | 1 | 2 = 0;
  private complete = false;

  constructor(options: WebNoiseHandshakeOptions) {
    this.options = options;
    this.localStatic = options.localIdentity.publicKey;
    this.responderStatic = decodeBase64Url(options.responderStaticKey, "responderStaticKey");
    this.h = sha256(PROTOCOL_NAME);
    this.ck = this.h;
    this.mixHash(options.prologue);
    this.mixHash(this.responderStatic);
  }

  get isComplete(): boolean {
    return this.complete;
  }

  writeMessage(): Uint8Array {
    if (this.complete) throw new RemoteError("noise_state", "握手已完成");
    if (this.options.initiator) {
      if (this.step !== 0) throw new RemoteError("noise_state", "Initiator 只能写第一条消息");
      const ephemeral = this.options.ephemeral ?? generateWebIdentity();
      this.ephemeral = ephemeral;
      this.mixHash(ephemeral.publicKey);
      this.mixKey(sharedSecret(ephemeral.privateKey, this.responderStatic));
      const encryptedStatic = this.encryptAndHash(this.localStatic);
      this.mixKey(sharedSecret(this.options.localIdentity.privateKey, this.responderStatic));
      this.step = 1;
      return concat(ephemeral.publicKey, encryptedStatic);
    }
    if (this.step !== 1 || !this.remoteEphemeral || !this.peerStatic) {
      throw new RemoteError("noise_state", "Responder 必须先读取第一条消息");
    }
    const ephemeral = this.options.ephemeral ?? generateWebIdentity();
    this.ephemeral = ephemeral;
    this.mixHash(ephemeral.publicKey);
    this.mixKey(sharedSecret(ephemeral.privateKey, this.remoteEphemeral));
    // IK msg2 的 se = DH(initiator static, responder ephemeral)。
    this.mixKey(sharedSecret(ephemeral.privateKey, this.peerStatic));
    this.step = 2;
    this.complete = true;
    return ephemeral.publicKey;
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
      this.remoteEphemeral = message;
      this.mixHash(message);
      if (!this.ephemeral) throw new RemoteError("noise_state", "缺少 ephemeral key");
      this.mixKey(sharedSecret(this.ephemeral.privateKey, message));
      this.mixKey(sharedSecret(this.options.localIdentity.privateKey, message));
      this.step = 2;
      this.complete = true;
      return;
    }
    if (this.step !== 0) throw new RemoteError("noise_state", "Responder 只能读取第一条消息");
    if (message.length !== HASH_BYTES * 2 + TAG_BYTES) {
      throw new RemoteError("noise_message", "Initiator 握手消息长度非法");
    }
    const remoteEphemeral = message.subarray(0, HASH_BYTES);
    this.remoteEphemeral = remoteEphemeral;
    this.mixHash(remoteEphemeral);
    this.mixKey(sharedSecret(this.options.localIdentity.privateKey, remoteEphemeral));
    const peerStatic = this.decryptAndHash(message.subarray(HASH_BYTES));
    if (peerStatic.length !== HASH_BYTES) {
      throw new RemoteError("noise_message", "Initiator static key 长度非法");
    }
    const expected = this.options.expectedPeerStaticKey;
    if (expected) {
      const expectedRaw = decodeBase64Url(expected, "expectedPeerStaticKey");
      if (!equalBytes(expectedRaw, peerStatic)) {
        throw new RemoteError("identity_mismatch", "Noise 对端 static key 与固定身份不一致");
      }
    }
    this.peerStatic = peerStatic;
    this.mixKey(sharedSecret(this.options.localIdentity.privateKey, peerStatic));
    this.step = 1;
  }

  split(): WebNoiseTransportPair {
    if (!this.complete) throw new RemoteError("noise_state", "握手未完成，不能 split");
    const okm = hkdf(sha256, new Uint8Array(HASH_BYTES), this.ck, EMPTY, KEY_BYTES * 2);
    const k1 = okm.slice(0, KEY_BYTES);
    const k2 = okm.slice(KEY_BYTES, KEY_BYTES * 2);
    return this.options.initiator
      ? { sendKey: k1, receiveKey: k2 }
      : { sendKey: k2, receiveKey: k1 };
  }

  private mixHash(data: Uint8Array): void {
    this.h = sha256(concat(this.h, data));
  }

  private mixKey(inputKeyMaterial: Uint8Array): void {
    const okm = hkdf(sha256, inputKeyMaterial, this.ck, EMPTY, KEY_BYTES * 2);
    this.ck = okm.slice(0, KEY_BYTES);
    this.k = okm.slice(KEY_BYTES, KEY_BYTES * 2);
    this.nonce = 0n;
  }

  private encryptAndHash(plaintext: Uint8Array): Uint8Array {
    if (!this.k) throw new RemoteError("noise_state", "缺少握手密钥");
    const ciphertext = encryptWithAd(this.k, this.nonce, this.h, plaintext);
    this.nonce += 1n;
    this.mixHash(ciphertext);
    return ciphertext;
  }

  private decryptAndHash(ciphertext: Uint8Array): Uint8Array {
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

export class WebNoiseTransport {
  private sendKey: Uint8Array;
  private receiveKey: Uint8Array;
  private sendNonce = 0n;
  private receiveNonce = 0n;
  private failed = false;

  constructor(pair: WebNoiseTransportPair) {
    this.sendKey = pair.sendKey;
    this.receiveKey = pair.receiveKey;
  }

  send(plaintext: Uint8Array): Uint8Array {
    this.assertUsable();
    if (plaintext.length + TAG_BYTES > MAX_NOISE_MESSAGE_BYTES) {
      throw new RemoteError("noise_message_too_large", "Noise transport 明文超过上限");
    }
    const ciphertext = encryptWithAd(this.sendKey, this.sendNonce, EMPTY, plaintext);
    this.sendNonce += 1n;
    return ciphertext;
  }

  receive(ciphertext: Uint8Array): Uint8Array {
    this.assertUsable();
    if (ciphertext.length > MAX_NOISE_MESSAGE_BYTES) {
      this.failed = true;
      throw new RemoteError("noise_message_too_large", "Noise transport 密文超过上限");
    }
    try {
      const plaintext = decryptWithAd(this.receiveKey, this.receiveNonce, EMPTY, ciphertext);
      this.receiveNonce += 1n;
      return plaintext;
    } catch (error) {
      this.failed = true;
      throw new RemoteError("noise_auth_failed", "Noise transport 认证失败", { cause: error });
    }
  }

  private assertUsable(): void {
    if (this.failed) throw new RemoteError("noise_failed", "Noise transport 已失效");
  }
}

function concat(...parts: Uint8Array[]): Uint8Array {
  const total = parts.reduce((sum, part) => sum + part.length, 0);
  const result = new Uint8Array(total);
  let offset = 0;
  for (const part of parts) {
    result.set(part, offset);
    offset += part.length;
  }
  return result;
}

function equalBytes(left: Uint8Array, right: Uint8Array): boolean {
  if (left.length !== right.length) return false;
  let diff = 0;
  for (let index = 0; index < left.length; index += 1) diff |= left[index]! ^ right[index]!;
  return diff === 0;
}

function sharedSecret(privateKey: Uint8Array, publicKey: Uint8Array): Uint8Array {
  if (publicKey.length !== KEY_BYTES) {
    throw new RemoteError("invalid_identity_key", "X25519 公钥必须为 32 字节");
  }
  const shared = x25519.getSharedSecret(privateKey, publicKey);
  if (shared.length !== KEY_BYTES || shared.every((byte) => byte === 0)) {
    throw new RemoteError("invalid_shared_secret", "X25519 共享密钥非法");
  }
  return shared;
}

function nonceBytes(counter: bigint): Uint8Array {
  const nonce = new Uint8Array(12);
  new DataView(nonce.buffer).setBigUint64(4, counter, true);
  return nonce;
}

function encryptWithAd(
  key: Uint8Array,
  nonce: bigint,
  ad: Uint8Array,
  plaintext: Uint8Array,
): Uint8Array {
  return chacha20poly1305(key, nonceBytes(nonce), ad).encrypt(plaintext);
}

function decryptWithAd(
  key: Uint8Array,
  nonce: bigint,
  ad: Uint8Array,
  ciphertext: Uint8Array,
): Uint8Array {
  if (ciphertext.length < TAG_BYTES) {
    throw new RemoteError("noise_message", "Noise 密文短于认证标签");
  }
  return chacha20poly1305(key, nonceBytes(nonce), ad).decrypt(ciphertext);
}
