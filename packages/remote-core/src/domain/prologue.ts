import { invalid } from "./errors.js";
import { PROTOCOL_VERSION } from "./limits.js";

export interface ConnectionContext {
  connectionId: string;
  hostDeviceId: string;
  clientDeviceId: string;
}

/** Noise prologue 绑定协议版本、连接与双方设备 ID，防止握手被移植到其他连接。 */
export function buildPrologue(context: ConnectionContext): Uint8Array {
  const parts: Buffer[] = [Buffer.from("MODE-REMOTE-V3", "utf8")];
  const version = Buffer.alloc(2);
  version.writeUInt16BE(PROTOCOL_VERSION, 0);
  parts.push(version);
  for (const value of [context.connectionId, context.hostDeviceId, context.clientDeviceId]) {
    const bytes = Buffer.from(value, "utf8");
    if (bytes.length < 1 || bytes.length > 64) invalid("prologue 字段长度非法");
    const length = Buffer.alloc(2);
    length.writeUInt16BE(bytes.length, 0);
    parts.push(length, bytes);
  }
  return Buffer.concat(parts);
}
