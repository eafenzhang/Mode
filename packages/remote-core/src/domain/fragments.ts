import { invalid } from "./errors.js";
import {
  MAX_CONCURRENT_REASSEMBLIES,
  MAX_SECURE_MESSAGE_BYTES,
  SECURE_FRAGMENT_PAYLOAD_BYTES,
} from "./limits.js";

const MAGIC = Buffer.from("DSHF", "ascii");
const VERSION = 1;
const HEADER_BYTES = 17;

export interface FragmentHeader {
  messageId: number;
  index: number;
  total: number;
  totalBytes: number;
}

/** 把完整业务消息切成 DSHF 分片。空消息也产生一个分片。 */
export function fragmentMessage(messageId: number, plaintext: Uint8Array): Uint8Array[] {
  if (!Number.isSafeInteger(messageId) || messageId < 0 || messageId > 0xffffffff) {
    invalid("fragment messageId 必须是 u32");
  }
  if (plaintext.length > MAX_SECURE_MESSAGE_BYTES) {
    invalid("分片前消息超过 4 MiB 上限");
  }
  const total = Math.max(1, Math.ceil(plaintext.length / SECURE_FRAGMENT_PAYLOAD_BYTES));
  if (total > 0xffff) invalid("分片数量超过 u16 上限");
  const fragments: Uint8Array[] = [];
  for (let index = 0; index < total; index += 1) {
    const start = index * SECURE_FRAGMENT_PAYLOAD_BYTES;
    const payload = plaintext.subarray(
      start,
      Math.min(plaintext.length, start + SECURE_FRAGMENT_PAYLOAD_BYTES),
    );
    fragments.push(encodeFragment(messageId, index, total, plaintext.length, payload));
  }
  return fragments;
}

function encodeFragment(
  messageId: number,
  index: number,
  total: number,
  totalBytes: number,
  payload: Uint8Array,
): Uint8Array {
  const header = Buffer.alloc(HEADER_BYTES);
  MAGIC.copy(header, 0);
  header.writeUInt8(VERSION, 4);
  header.writeUInt32BE(messageId, 5);
  header.writeUInt16BE(index, 9);
  header.writeUInt16BE(total, 11);
  header.writeUInt32BE(totalBytes, 13);
  return Buffer.concat([header, Buffer.from(payload)]);
}

export function isFragmentPayload(bytes: Uint8Array): boolean {
  return bytes.length >= HEADER_BYTES && Buffer.from(bytes.subarray(0, 4)).equals(MAGIC);
}

export function decodeFragmentHeader(fragment: Uint8Array): FragmentHeader {
  if (fragment.length < HEADER_BYTES) invalid("分片短于头部");
  const buffer = Buffer.from(fragment);
  if (!buffer.subarray(0, 4).equals(MAGIC)) invalid("分片 magic 不匹配");
  if (buffer.readUInt8(4) !== VERSION) invalid("分片版本不匹配");
  return {
    messageId: buffer.readUInt32BE(5),
    index: buffer.readUInt16BE(9),
    total: buffer.readUInt16BE(11),
    totalBytes: buffer.readUInt32BE(13),
  };
}

interface PendingMessage {
  total: number;
  totalBytes: number;
  nextIndex: number;
  chunks: Buffer[];
  received: number;
}

/** 同一安全通道内的 DSHF 重组器。重复、乱序、长度不一致一律 fail closed。 */
export class FragmentReassembler {
  private readonly pending = new Map<number, PendingMessage>();

  push(fragment: Uint8Array): { messageId: number; plaintext: Uint8Array } | null {
    if (fragment.length > HEADER_BYTES + SECURE_FRAGMENT_PAYLOAD_BYTES) {
      invalid("分片 payload 超过 48 KiB");
    }
    const header = decodeFragmentHeader(fragment);
    if (header.total < 1) invalid("分片 total 非法");
    if (header.index >= header.total) invalid("分片 index 越界");
    if (header.totalBytes > MAX_SECURE_MESSAGE_BYTES) invalid("分片声明消息超过 4 MiB");
    const payload = Buffer.from(fragment.subarray(HEADER_BYTES));
    let pending = this.pending.get(header.messageId);
    if (pending) {
      if (pending.total !== header.total || pending.totalBytes !== header.totalBytes) {
        invalid("分片元数据不一致");
      }
      if (header.index !== pending.nextIndex) invalid("分片乱序或重复");
    } else {
      if (header.index !== 0) invalid("分片未从 index 0 开始");
      if (this.pending.size >= MAX_CONCURRENT_REASSEMBLIES) {
        invalid("并行重组消息超过 8 条");
      }
      pending = {
        total: header.total,
        totalBytes: header.totalBytes,
        nextIndex: 0,
        chunks: [],
        received: 0,
      };
      this.pending.set(header.messageId, pending);
    }
    const expectedLastBytes =
      header.totalBytes - (header.total - 1) * SECURE_FRAGMENT_PAYLOAD_BYTES;
    const expectedBytes =
      header.index === header.total - 1 ? expectedLastBytes : SECURE_FRAGMENT_PAYLOAD_BYTES;
    if (header.totalBytes === 0) {
      if (payload.length !== 0) invalid("空消息分片必须为空");
    } else if (payload.length !== expectedBytes) {
      invalid("分片长度与 totalBytes 不一致");
    }
    pending.chunks.push(payload);
    pending.received += payload.length;
    pending.nextIndex += 1;
    if (pending.nextIndex < pending.total) return null;
    if (pending.received !== pending.totalBytes) invalid("分片重组长度不一致");
    this.pending.delete(header.messageId);
    return { messageId: header.messageId, plaintext: Buffer.concat(pending.chunks) };
  }

  clear(): void {
    this.pending.clear();
  }
}
