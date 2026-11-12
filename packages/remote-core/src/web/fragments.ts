import { RemoteError } from "../domain/errors.js";
import {
  MAX_CONCURRENT_REASSEMBLIES,
  MAX_SECURE_MESSAGE_BYTES,
  SECURE_FRAGMENT_PAYLOAD_BYTES,
} from "../domain/limits.js";

/**
 * 浏览器侧的 DSHF 分片（与 `domain/fragments.ts` 同协议）。
 *
 * 为什么不直接复用：Node 版依赖 `Buffer`，浏览器入口不能引入 Node 内置模块。
 * Noise 单条消息上限 64KB，而业务消息允许 4MB；浏览器必须与 SecureSession
 * 一样在明文层分片，否则：
 * - Host → 浏览器超过 48KB 的响应到达时是 DSHF 片段，浏览器无法解析；
 * - 浏览器 → Host 超过 64KB 的消息会被 `WebNoiseTransport.send` 拒绝。
 */
const HEADER_BYTES = 17;
const VERSION = 1;
const MAGIC = [0x44, 0x53, 0x48, 0x46] as const; // "DSHF"

export function fragmentWebMessage(messageId: number, plaintext: Uint8Array): Uint8Array[] {
  if (!Number.isSafeInteger(messageId) || messageId < 0 || messageId > 0xffffffff) {
    throw new RemoteError("invalid_fragment", "分片 messageId 必须是 u32");
  }
  if (plaintext.length > MAX_SECURE_MESSAGE_BYTES) {
    throw new RemoteError("message_too_large", "分片前消息超过 4 MiB 上限");
  }
  const total = Math.max(1, Math.ceil(plaintext.length / SECURE_FRAGMENT_PAYLOAD_BYTES));
  if (total > 0xffff) throw new RemoteError("invalid_fragment", "分片数量超过 u16 上限");
  const fragments: Uint8Array[] = [];
  for (let index = 0; index < total; index += 1) {
    const start = index * SECURE_FRAGMENT_PAYLOAD_BYTES;
    const end = Math.min(plaintext.length, start + SECURE_FRAGMENT_PAYLOAD_BYTES);
    const fragment = new Uint8Array(HEADER_BYTES + (end - start));
    const view = new DataView(fragment.buffer);
    fragment.set(MAGIC, 0);
    view.setUint8(4, VERSION);
    view.setUint32(5, messageId);
    view.setUint16(9, index);
    view.setUint16(11, total);
    view.setUint32(13, plaintext.length);
    fragment.set(plaintext.subarray(start, end), HEADER_BYTES);
    fragments.push(fragment);
  }
  return fragments;
}

export function isWebFragment(bytes: Uint8Array): boolean {
  return (
    bytes.length >= HEADER_BYTES &&
    bytes[0] === MAGIC[0] &&
    bytes[1] === MAGIC[1] &&
    bytes[2] === MAGIC[2] &&
    bytes[3] === MAGIC[3]
  );
}

interface PendingMessage {
  total: number;
  totalBytes: number;
  nextIndex: number;
  chunks: Uint8Array[];
  received: number;
}

/** 与 Node `FragmentReassembler` 同口径：重复、乱序、长度不一致一律 fail closed。 */
export class WebFragmentReassembler {
  private readonly pending = new Map<number, PendingMessage>();

  push(fragment: Uint8Array): { messageId: number; plaintext: Uint8Array } | null {
    if (fragment.length > HEADER_BYTES + SECURE_FRAGMENT_PAYLOAD_BYTES) {
      throw new RemoteError("invalid_fragment", "分片 payload 超过 48 KiB");
    }
    if (!isWebFragment(fragment)) throw new RemoteError("invalid_fragment", "分片 magic 不匹配");
    const view = new DataView(fragment.buffer, fragment.byteOffset, fragment.byteLength);
    if (view.getUint8(4) !== VERSION) {
      throw new RemoteError("invalid_fragment", "分片版本不匹配");
    }
    const messageId = view.getUint32(5);
    const index = view.getUint16(9);
    const total = view.getUint16(11);
    const totalBytes = view.getUint32(13);
    if (total < 1 || index >= total) throw new RemoteError("invalid_fragment", "分片索引非法");
    if (totalBytes > MAX_SECURE_MESSAGE_BYTES) {
      throw new RemoteError("invalid_fragment", "分片声明消息超过 4 MiB");
    }
    const payload = fragment.subarray(HEADER_BYTES);
    let pending = this.pending.get(messageId);
    if (pending) {
      if (pending.total !== total || pending.totalBytes !== totalBytes) {
        throw new RemoteError("invalid_fragment", "分片元数据不一致");
      }
      if (index !== pending.nextIndex) {
        throw new RemoteError("invalid_fragment", "分片乱序或重复");
      }
    } else {
      if (index !== 0) throw new RemoteError("invalid_fragment", "分片未从 index 0 开始");
      if (this.pending.size >= MAX_CONCURRENT_REASSEMBLIES) {
        throw new RemoteError("invalid_fragment", "并行重组消息超过 8 条");
      }
      pending = { total, totalBytes, nextIndex: 0, chunks: [], received: 0 };
      this.pending.set(messageId, pending);
    }
    const expectedLast = totalBytes - (total - 1) * SECURE_FRAGMENT_PAYLOAD_BYTES;
    const expected = index === total - 1 ? expectedLast : SECURE_FRAGMENT_PAYLOAD_BYTES;
    if (totalBytes === 0) {
      if (payload.length !== 0) throw new RemoteError("invalid_fragment", "空消息分片必须为空");
    } else if (payload.length !== expected) {
      throw new RemoteError("invalid_fragment", "分片长度与 totalBytes 不一致");
    }
    pending.chunks.push(payload);
    pending.received += payload.length;
    pending.nextIndex += 1;
    if (pending.nextIndex < pending.total) return null;
    if (pending.received !== pending.totalBytes) {
      throw new RemoteError("invalid_fragment", "分片重组长度不一致");
    }
    this.pending.delete(messageId);
    const plaintext = new Uint8Array(pending.received);
    let offset = 0;
    for (const chunk of pending.chunks) {
      plaintext.set(chunk, offset);
      offset += chunk.length;
    }
    return { messageId, plaintext };
  }

  clear(): void {
    this.pending.clear();
  }
}
