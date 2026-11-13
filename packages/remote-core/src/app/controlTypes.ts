import type { BusinessFrame } from "../domain/businessFrames.js";
import type { DeviceDescriptor } from "../domain/descriptor.js";
import type { RemoteError } from "../domain/errors.js";
import type { IdentityKeyPair } from "../adapters/identity.js";

export interface RemoteSocket {
  send(text: string): void;
  close(code?: number, reason?: string): void;
  onMessage(callback: (text: string) => void): void;
  onClose(callback: (code: number, reason: string) => void): void;
  /**
   * 数据面发送队列水位（字节）：WS 用 `bufferedAmount`，werift 用 DataChannel 同名属性。
   * 可选：测试桩/旧适配器不实现时上层按 0 处理（= 不丢帧，保持旧行为）。
   * 媒体面靠它在真正积压的地方丢旧视频帧（spec: remote-desktop-latency §2.3）。
   */
  readonly bufferedAmount?: number;
}

export interface RemoteConnection {
  readonly connectionId: string;
  readonly peer: DeviceDescriptor;
  readonly role: "host" | "client";
  /** 当前数据面 socket 的发送队列水位；未选路/未 attach 时为 0。 */
  readonly bufferedAmount: number;
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
  /** Client 选路完成后回调（p2p/lan/relay）；用于观测与测试。 */
  onTransport?: (transport: import("../domain/controlMessages.js").TransportKind) => void;
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
  /**
   * Client/Host 尝试 WebRTC P2P：提供适配器后连接先协商 p2p，失败回落 Relay。
   * 两端都必须提供；只有一端提供时对端会在超时后回落。
   */
  p2p?: {
    adapter: import("./p2p.js").P2PAdapter;
    /** p2p 协商总超时（含 offer/answer/ICE）；缺省 8000ms。 */
    timeoutMs?: number;
  };
  clientVersion?: string;
  connectTimeoutMs?: number;
}
