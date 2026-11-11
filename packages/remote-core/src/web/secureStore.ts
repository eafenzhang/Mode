import type { WebCredentialStore } from "./accountTypes.js";
import { decodeBase64Url, encodeBase64Url } from "./base64url.js";

/** 不可导出 AES-GCM 密钥在对象存储里的键；与 profile 分开存放。 */
const STORE_KEY_ID = "mode.remote.v3.store-key";
const ENVELOPE_VERSION = 1;
const IV_BYTES = 12;

interface Envelope {
  v: number;
  iv: string;
  data: string;
}

/** 复制到 ArrayBuffer 背书的视图：WebCrypto 不接受 SharedArrayBuffer 可能性的泛型。 */
function toBufferSource(bytes: Uint8Array): Uint8Array<ArrayBuffer> {
  const copy = new Uint8Array(bytes.byteLength);
  copy.set(bytes);
  return copy;
}

function parseEnvelope(raw: string): Envelope | null {
  try {
    const value = JSON.parse(raw) as Partial<Envelope>;
    if (
      value.v !== ENVELOPE_VERSION ||
      typeof value.iv !== "string" ||
      typeof value.data !== "string"
    ) {
      return null;
    }
    return { v: value.v, iv: value.iv, data: value.data };
  } catch {
    return null;
  }
}

/**
 * 给凭据存储加一层 AES-GCM（docs/specs/remote-control-v3-web.md 安全节）。
 *
 * - 密钥是 `extractable=false` 的 AES-GCM CryptoKey，经对象存储持久化；JS 无法导出原始
 *   字节，避免 IndexedDB/磁盘转储直接拿到私钥与 token。
 * - 旧明文值读取兼容，下一次写入自动升级为密文信封。
 * - 存储不支持对象持久化（localStorage 回落）时原样返回 inner：加密不可用而不是假装加密。
 */
export function createEncryptedCredentialStore(inner: WebCredentialStore): WebCredentialStore {
  const readValue = inner.getValue;
  const writeValue = inner.setValue;
  if (typeof readValue !== "function" || typeof writeValue !== "function") {
    return inner;
  }

  let keyPromise: Promise<CryptoKey> | null = null;
  const ensureKey = (): Promise<CryptoKey> => {
    keyPromise ??= (async () => {
      const existing = await readValue.call(inner, STORE_KEY_ID);
      if (existing) return existing as CryptoKey;
      const generated = (await crypto.subtle.generateKey({ name: "AES-GCM", length: 256 }, false, [
        "encrypt",
        "decrypt",
      ])) as CryptoKey;
      await writeValue.call(inner, STORE_KEY_ID, generated);
      return generated;
    })();
    return keyPromise;
  };

  return {
    async get(key) {
      const raw = await inner.get(key);
      if (raw === null) return null;
      const envelope = parseEnvelope(raw);
      // 旧版本明文：直接返回；下一次 set 时升级为密文。
      if (!envelope) return raw;
      const keyMaterial = await ensureKey();
      const plaintext = await crypto.subtle.decrypt(
        {
          name: "AES-GCM",
          iv: toBufferSource(decodeBase64Url(envelope.iv, "store.iv")),
        },
        keyMaterial,
        toBufferSource(decodeBase64Url(envelope.data, "store.data")),
      );
      return new TextDecoder().decode(plaintext);
    },
    async set(key, value) {
      const keyMaterial = await ensureKey();
      const iv = crypto.getRandomValues(new Uint8Array(IV_BYTES));
      const encrypted = await crypto.subtle.encrypt(
        { name: "AES-GCM", iv },
        keyMaterial,
        new TextEncoder().encode(value),
      );
      const envelope: Envelope = {
        v: ENVELOPE_VERSION,
        iv: encodeBase64Url(iv),
        data: encodeBase64Url(new Uint8Array(encrypted)),
      };
      await inner.set(key, JSON.stringify(envelope));
    },
    delete(key) {
      return inner.delete(key);
    },
  };
}
