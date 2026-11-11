import { connectViaProtocolWithClient } from "@mode/client";
import { decodeBase64Url, encodeBase64Url } from "@mode/remote-core";
import { Emitter, SocketProtocol, VSBuffer, type ISocket } from "@mode/rpc";
import type { IRemoteV3Service } from "@mode/services";
import {
  MODE_RPC_HOST_CAPABILITY_HEADER,
  serverRemoteInfoSchema,
  type ServerRemoteInfo,
} from "@mode/shared";

const CAPABILITY_TIMEOUT_MS = 15_000;
const RECONNECT_BASE_MS = 300;
const RECONNECT_MAX_MS = 5_000;
const MAX_RECONNECT_ATTEMPTS = 10;

interface WorkspaceTunnelEndpoint {
  send(bytes: Uint8Array): void;
  close(): void;
  onData(callback: (bytes: Uint8Array) => void): void;
  onClose(callback: () => void): void;
}

function decodeBody(bodyBase64: string): unknown {
  return JSON.parse(Buffer.from(bodyBase64, "base64").toString("utf8"));
}

/** 每次重连都重新申请一次性 capability，再打开 `/ws/host` 隧道。 */
async function openWorkspaceTunnelEndpoint(
  remoteV3: IRemoteV3Service,
  hostDeviceId: string,
): Promise<WorkspaceTunnelEndpoint> {
  const capabilityResponse = await remoteV3.httpRequest({
    hostDeviceId,
    method: "POST",
    path: "/api/rpc-host-capability",
    timeoutMs: CAPABILITY_TIMEOUT_MS,
  });
  if (capabilityResponse.status !== 200) {
    throw new Error(`申请对端连接票据失败（HTTP ${capabilityResponse.status}）`);
  }
  const capabilityPayload = decodeBody(capabilityResponse.bodyBase64) as { capability?: unknown };
  if (typeof capabilityPayload.capability !== "string" || !capabilityPayload.capability) {
    throw new Error("对端未返回连接票据");
  }
  const { tunnelId } = await remoteV3.openTunnel({
    hostDeviceId,
    path: "/ws/host",
    headers: { [MODE_RPC_HOST_CAPABILITY_HEADER]: capabilityPayload.capability },
  });

  const dataListeners = new Set<(bytes: Uint8Array) => void>();
  const closeListeners = new Set<() => void>();
  const buffered: Uint8Array[] = [];
  let onData: ((bytes: Uint8Array) => void) | null = null;
  let closed = false;
  const emitClose = (): void => {
    if (closed) return;
    closed = true;
    subscription.dispose();
    for (const listener of closeListeners) listener();
    closeListeners.clear();
  };
  const subscription = remoteV3.onTunnelEvent((event) => {
    if (event.tunnelId !== tunnelId) return;
    if (event.kind === "message") {
      if (!event.data) return;
      const frame = decodeBase64Url(event.data, "v3.tunnel.data");
      if (frame.length === 0 || frame[0] !== 1) return;
      const bytes = frame.subarray(1);
      if (onData) onData(bytes);
      else buffered.push(bytes);
      return;
    }
    if (event.kind === "close") {
      emitClose();
    }
  });

  return {
    send(bytes) {
      if (closed) return;
      const frame = new Uint8Array(bytes.byteLength + 1);
      frame[0] = 1;
      frame.set(bytes, 1);
      void remoteV3.sendTunnel(tunnelId, encodeBase64Url(frame)).catch(() => emitClose());
    },
    close() {
      if (closed) return;
      emitClose();
      void remoteV3.closeTunnel(tunnelId).catch(() => undefined);
    },
    onData(callback) {
      onData = callback;
      dataListeners.add(callback);
      for (const bytes of buffered.splice(0, buffered.length)) callback(bytes);
    },
    onClose(callback) {
      closeListeners.add(callback);
      if (closed) queueMicrotask(callback);
    },
  };
}

/**
 * 可续传的 `/ws/host` socket：传输断开时保留外层 ISocket 与 ChannelClient，
 * 重新申请 capability、打开新隧道；新 ChannelServer 的 Initialize 触发
 * ChannelClient 重放事件订阅。
 */
class ReconnectingWorkspaceSocket implements ISocket {
  private readonly onDataEmitter = new Emitter<VSBuffer>();
  private readonly onCloseEmitter = new Emitter<void>();
  private readonly onEndEmitter = new Emitter<void>();
  private endpoint: WorkspaceTunnelEndpoint | null = null;
  private reconnectTimer: NodeJS.Timeout | null = null;
  private attempt = 0;
  private disposed = false;
  private queue: Uint8Array[] = [];
  private queuedBytes = 0;

  constructor(
    private readonly options: {
      open: () => Promise<WorkspaceTunnelEndpoint>;
      onTransportLost: () => void;
      onFatal: () => void;
    },
  ) {}

  async start(): Promise<void> {
    const endpoint = await this.options.open();
    if (this.disposed) {
      endpoint.close();
      return;
    }
    this.endpoint = endpoint;
    this.bind(endpoint);
  }

  readonly onData = this.onDataEmitter.event;
  readonly onClose = this.onCloseEmitter.event;
  readonly onEnd = this.onEndEmitter.event;

  write(buffer: VSBuffer): void {
    const bytes = new Uint8Array(buffer.buffer);
    if (this.endpoint) {
      this.endpoint.send(bytes);
      return;
    }
    if (this.queue.length >= 512 || this.queuedBytes + bytes.byteLength > 4 * 1024 * 1024) {
      this.fail();
      return;
    }
    this.queue.push(bytes);
    this.queuedBytes += bytes.byteLength;
  }

  end(): void {
    this.dispose();
  }

  drain(): Promise<void> {
    return Promise.resolve();
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.clearReconnectTimer();
    this.endpoint?.close();
    this.endpoint = null;
    this.queue = [];
    this.queuedBytes = 0;
    this.onCloseEmitter.fire();
    this.onEndEmitter.fire();
  }

  private bind(endpoint: WorkspaceTunnelEndpoint): void {
    endpoint.onData((bytes) => {
      this.onDataEmitter.fire(VSBuffer.wrap(bytes));
    });
    endpoint.onClose(() => {
      if (this.endpoint !== endpoint || this.disposed) return;
      this.endpoint = null;
      this.options.onTransportLost();
      this.scheduleReconnect();
    });
    for (const bytes of this.queue.splice(0, this.queue.length)) endpoint.send(bytes);
    this.queuedBytes = 0;
  }

  private scheduleReconnect(): void {
    if (this.disposed || this.reconnectTimer) return;
    this.attempt += 1;
    if (this.attempt > MAX_RECONNECT_ATTEMPTS) {
      this.fail();
      return;
    }
    const base = Math.min(RECONNECT_BASE_MS * 2 ** (this.attempt - 1), RECONNECT_MAX_MS);
    const delay = Math.max(RECONNECT_BASE_MS, Math.round(base * (0.5 + Math.random() * 0.5)));
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      void this.reconnect();
    }, delay);
    this.reconnectTimer.unref?.();
  }

  private async reconnect(): Promise<void> {
    try {
      const endpoint = await this.options.open();
      if (this.disposed) {
        endpoint.close();
        return;
      }
      this.endpoint = endpoint;
      this.attempt = 0;
      this.bind(endpoint);
    } catch {
      this.scheduleReconnect();
    }
  }

  private fail(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.clearReconnectTimer();
    this.endpoint?.close();
    this.endpoint = null;
    this.options.onFatal();
    this.onCloseEmitter.fire();
    this.onEndEmitter.fire();
  }

  private clearReconnectTimer(): void {
    if (!this.reconnectTimer) return;
    clearTimeout(this.reconnectTimer);
    this.reconnectTimer = null;
  }
}

export interface V3WorkspaceAttachment {
  services: ReturnType<typeof connectViaProtocolWithClient>["accessor"];
  info: ServerRemoteInfo;
  dispose(): Promise<void>;
}

function decodeInfo(bodyBase64: string): unknown {
  return JSON.parse(Buffer.from(bodyBase64, "base64").toString("utf8"));
}

/**
 * v3 远程工作区挂接：读取对端能力位 → 申请一次性 host capability →
 * 打开到对端 `/ws/host` 的 E2EE WebSocket 隧道 → 复用 LAN 同一套
 * `SocketProtocol → ChannelClient → RemoteServiceAccess`。
 *
 * 传输断开时保持 services 引用不变，自动重开隧道并重建 ChannelServer 会话。
 */
export async function attachV3RemoteWorkspace(params: {
  remoteV3: IRemoteV3Service;
  hostDeviceId: string;
  signal?: AbortSignal;
  onClose?: () => void;
}): Promise<V3WorkspaceAttachment> {
  const { remoteV3, hostDeviceId, signal } = params;
  if (signal?.aborted) {
    throw new Error("远程连接已取消");
  }

  const infoResponse = await remoteV3.httpRequest({
    hostDeviceId,
    method: "GET",
    path: "/api/server-info",
    timeoutMs: CAPABILITY_TIMEOUT_MS,
  });
  if (infoResponse.status !== 200) {
    throw new Error(`读取对端信息失败（HTTP ${infoResponse.status}）`);
  }
  const info = serverRemoteInfoSchema.parse(decodeInfo(infoResponse.bodyBase64));

  let protocol!: SocketProtocol;
  let channelClient: ReturnType<typeof connectViaProtocolWithClient>["client"] | null = null;
  const socket = new ReconnectingWorkspaceSocket({
    open: () => openWorkspaceTunnelEndpoint(remoteV3, hostDeviceId),
    onTransportLost: () => {
      // 旧连接可能停在半帧；清掉分片缓冲再由 ChannelClient 失败在途请求。
      protocol.reset();
      channelClient?.notifyTransportClosed(new Error("远程工作区隧道重连中，请重试"));
    },
    onFatal: () => params.onClose?.(),
  });

  // wrap / SocketProtocol / ChannelClient 必须在任何数据帧到达前同步建好：
  // ChannelServer 建连即发 `Initialize`，错过这一帧 ChannelClient 会永久停在 Uninitialized。
  protocol = new SocketProtocol(socket);
  const connected = connectViaProtocolWithClient(protocol);
  channelClient = connected.client;
  try {
    await socket.start();
  } catch (error) {
    socket.dispose();
    throw error;
  }

  const abort = (): void => {
    socket.dispose();
  };
  signal?.addEventListener("abort", abort, { once: true });

  return {
    services: connected.accessor,
    info,
    async dispose() {
      signal?.removeEventListener("abort", abort);
      socket.dispose();
    },
  };
}
