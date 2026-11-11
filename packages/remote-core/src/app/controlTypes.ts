import type { BusinessFrame } from "../domain/businessFrames.js";
import type { DeviceDescriptor } from "../domain/descriptor.js";
import type { RemoteError } from "../domain/errors.js";
import type { IdentityKeyPair } from "../adapters/identity.js";

export interface RemoteSocket {
  send(text: string): void;
  close(code?: number, reason?: string): void;
  onMessage(callback: (text: string) => void): void;
  onClose(callback: (code: number, reason: string) => void): void;
}

export interface RemoteConnection {
  readonly connectionId: string;
  readonly peer: DeviceDescriptor;
  readonly role: "host" | "client";
  send(frame: BusinessFrame): void;
  onFrame(callback: (frame: BusinessFrame) => void): () => void;
  onClose(callback: () => void): () => void;
  close(reason?: string): void;
}

export interface ControlLinkOptions {
  role: "host" | "client";
  serverUrl: string;
  deviceId: string;
  accessToken: string;
  identity: IdentityKeyPair;
  connectSocket: (url: string) => Promise<RemoteSocket>;
  fetchPeer: (deviceId: string) => Promise<DeviceDescriptor>;
  verifyPeer?: (peer: DeviceDescriptor) => boolean | Promise<boolean>;
  onConnection?: (connection: RemoteConnection) => void;
  onError?: (error: RemoteError) => void;
  /** Control WS 非主动关闭时回调；Host 用它触发重连，Client 用它清理缓存。 */
  onClose?: (details: { code: number; reason: string }) => void;
  /**
   * Client 尝试 LAN 直连：返回已完成 ticket 校验并消费 `relay.ready` 的 socket。
   * 缺省或失败时回落 Relay；Host 通过 `acceptDirectSocket` 接收对端直连。
   */
  openDirectSocket?: (
    direct: import("../domain/controlMessages.js").DirectAssignment,
    connectionId: string,
  ) => Promise<RemoteSocket>;
  clientVersion?: string;
  connectTimeoutMs?: number;
}
