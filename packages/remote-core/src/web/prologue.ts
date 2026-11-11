import { PROTOCOL_VERSION } from "../domain/limits.js";

export interface WebConnectionContext {
  connectionId: string;
  hostDeviceId: string;
  clientDeviceId: string;
}

/** 与 Node `buildPrologue` 逐字节一致的浏览器版本（不依赖 Buffer）。 */
export function buildWebPrologue(context: WebConnectionContext): Uint8Array {
  const encoder = new TextEncoder();
  const parts: Uint8Array[] = [encoder.encode("MODE-REMOTE-V3")];
  const version = new Uint8Array(2);
  new DataView(version.buffer).setUint16(0, PROTOCOL_VERSION, false);
  parts.push(version);
  for (const value of [context.connectionId, context.hostDeviceId, context.clientDeviceId]) {
    const bytes = encoder.encode(value);
    if (bytes.length < 1 || bytes.length > 64) {
      throw new Error("prologue 字段长度非法");
    }
    const length = new Uint8Array(2);
    new DataView(length.buffer).setUint16(0, bytes.length, false);
    parts.push(length, bytes);
  }
  const total = parts.reduce((sum, part) => sum + part.length, 0);
  const result = new Uint8Array(total);
  let offset = 0;
  for (const part of parts) {
    result.set(part, offset);
    offset += part.length;
  }
  return result;
}
