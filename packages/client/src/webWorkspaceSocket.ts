import type { WebBusinessStream, WebControlLink } from "@mode/remote-core/web";
import { Emitter, VSBuffer, type ISocket } from "@mode/rpc";
import { MODE_RPC_HOST_CAPABILITY_HEADER } from "@mode/shared";

import { decodeBase64 } from "./base64.js";

const CAPABILITY_TIMEOUT_MS = 15_000;
const RECONNECT_BASE_MS = 300;
const RECONNECT_MAX_MS = 5_000;
const MAX_RECONNECT_ATTEMPTS = 10;
const MAX_QUEUED_FRAMES = 512;
const MAX_QUEUED_BYTES = 4 * 1024 * 1024;

interface WorkspaceTunnelEndpoint {
  send(bytes: Uint8Array): void;
  close(): void;
  onData(callback: (bytes: Uint8Array) => void): void;
  onClose(callback: () => void): void;
}

/**
 * 隧道帧 → 端点：首字节 1=binary、0=text（RPC 只走 binary），与 Host `wsTunnel` 同口径。
 * 订阅前到达的数据先缓存，避免 Initialize 在 `SocketProtocol` 挂监听前丢失。
 */
function createStreamEndpoint(stream: WebBusinessStream): WorkspaceTunnelEndpoint {
  const closeListeners = new Set<() => void>();
  const buffered: Uint8Array[] = [];
  let onData: ((bytes: Uint8Array) => void) | null = null;
  let closed = false;
  const emitClose = (): void => {
    if (closed) return;
    closed = true;
    for (const listener of closeListeners) listener();
    closeListeners.clear();
  };

  stream.onData((frame) => {
    if (frame.length === 0 || frame[0] !== 1) return;
    const bytes = frame.subarray(1);
    if (onData) onData(bytes);
    else buffered.push(bytes);
  });
  stream.onClose(emitClose);

  return {
    send(bytes) {
      if (closed) return;
      const frame = new Uint8Array(bytes.byteLength + 1);
      frame[0] = 1;
      frame.set(bytes, 1);
      try {
        stream.send(frame);
      } catch {
        emitClose();
      }
    },
    close() {
      if (closed) return;
      try {
        stream.close("client disposed");
      } finally {
        emitClose();
      }
    },
    onData(callback) {
      onData = callback;
      for (const bytes of buffered.splice(0, buffered.length)) callback(bytes);
    },
    onClose(callback) {
      closeListeners.add(callback);
      if (closed) queueMicrotask(callback);
    },
  };
}

/** 每次重连都重新申请一次性 capability，再打开 `/ws/host` 隧道。 */
async function openWorkspaceEndpoint(
  link: Pick<WebControlLink, "request" | "openStream">,
  hostDeviceId: string,
  path: string,
  timeoutMs: number,
): Promise<WorkspaceTunnelEndpoint> {
  const response = (await link.request(
    "remote.http.request",
    { method: "POST", path: "/api/rpc-host-capability" },
    timeoutMs,
  )) as { status?: unknown; bodyBase64?: unknown };
  if (response.status !== 200 || typeof response.bodyBase64 !== "string") {
    throw new Error(`申请对端连接票据失败（HTTP ${String(response.status)}）`);
  }
  const payload = JSON.parse(new TextDecoder().decode(decodeBase64(response.bodyBase64))) as {
    capability?: unknown;
  };
  if (typeof payload.capability !== "string" || !payload.capability) {
    throw new Error("对端未返回连接票据");
  }
  const stream = link.openStream("remote.ws", {
    path,
    headers: { [MODE_RPC_HOST_CAPABILITY_HEADER]: payload.capability },
  });
  return createStreamEndpoint(stream);
}

export interface WebWorkspaceSocket {
  socket: ISocket;
  start(): Promise<void>;
  dispose(): void;
}

/**
 * 可续传的 `/ws/host` socket（浏览器侧）：传输断开时保留外层 ISocket 与 ChannelClient，
 * 重新申请 capability、打开新隧道；新 ChannelServer 的 Initialize 触发 ChannelClient
 * 重放事件订阅。与桌面 `workspaceAttach.ts` 的状态机同构，只是数据面换成 E2EE 流。
 */
export function createWebWorkspaceSocket(params: {
  link: Pick<WebControlLink, "request" | "openStream">;
  hostDeviceId: string;
  /** 目标 Host 本机路径；缺省 `/ws/host`。 */
  path?: string;
  timeoutMs?: number;
  /** 隧道断开（非本端 dispose）：调用方重置分片并拒绝在途 RPC。 */
  onTransportLost?: () => void;
  /** 重连退避耗尽：调用方标记会话断开。 */
  onFatal?: () => void;
}): WebWorkspaceSocket {
  const onDataEmitter = new Emitter<VSBuffer>();
  const onCloseEmitter = new Emitter<void>();
  const onEndEmitter = new Emitter<void>();
  const timeoutMs = params.timeoutMs ?? CAPABILITY_TIMEOUT_MS;
  const path = params.path ?? "/ws/host";
  let endpoint: WorkspaceTunnelEndpoint | null = null;
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  let attempt = 0;
  let disposed = false;
  let queuedBytes = 0;
  let queue: Uint8Array[] = [];

  const clearReconnectTimer = (): void => {
    if (!reconnectTimer) return;
    clearTimeout(reconnectTimer);
    reconnectTimer = null;
  };

  const emitClosed = (): void => {
    onCloseEmitter.fire();
    onEndEmitter.fire();
  };

  const fail = (): void => {
    if (disposed) return;
    disposed = true;
    clearReconnectTimer();
    endpoint?.close();
    endpoint = null;
    params.onFatal?.();
    emitClosed();
  };

  const scheduleReconnect = (): void => {
    if (disposed || reconnectTimer) return;
    attempt += 1;
    if (attempt > MAX_RECONNECT_ATTEMPTS) {
      fail();
      return;
    }
    const base = Math.min(RECONNECT_BASE_MS * 2 ** (attempt - 1), RECONNECT_MAX_MS);
    const delay = Math.max(RECONNECT_BASE_MS, Math.round(base * (0.5 + Math.random() * 0.5)));
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null;
      void reconnect();
    }, delay);
  };

  const bind = (next: WorkspaceTunnelEndpoint): void => {
    next.onData((bytes) => {
      onDataEmitter.fire(VSBuffer.wrap(bytes));
    });
    next.onClose(() => {
      if (endpoint !== next || disposed) return;
      endpoint = null;
      params.onTransportLost?.();
      scheduleReconnect();
    });
    for (const bytes of queue.splice(0, queue.length)) next.send(bytes);
    queuedBytes = 0;
  };

  const reconnect = async (): Promise<void> => {
    try {
      const next = await openWorkspaceEndpoint(params.link, params.hostDeviceId, path, timeoutMs);
      if (disposed) {
        next.close();
        return;
      }
      endpoint = next;
      attempt = 0;
      bind(next);
    } catch {
      scheduleReconnect();
    }
  };

  const socket: ISocket = {
    onData: onDataEmitter.event,
    onClose: onCloseEmitter.event,
    onEnd: onEndEmitter.event,
    write(buffer: VSBuffer) {
      const bytes = new Uint8Array(buffer.buffer);
      if (endpoint) {
        endpoint.send(bytes);
        return;
      }
      if (queue.length >= MAX_QUEUED_FRAMES || queuedBytes + bytes.byteLength > MAX_QUEUED_BYTES) {
        fail();
        return;
      }
      queue.push(bytes);
      queuedBytes += bytes.byteLength;
    },
    end() {
      socket.dispose();
    },
    drain() {
      return Promise.resolve();
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      clearReconnectTimer();
      endpoint?.close();
      endpoint = null;
      queue = [];
      queuedBytes = 0;
      emitClosed();
    },
  };

  return {
    socket,
    async start() {
      const first = await openWorkspaceEndpoint(params.link, params.hostDeviceId, path, timeoutMs);
      if (disposed) {
        first.close();
        return;
      }
      endpoint = first;
      bind(first);
    },
    dispose: () => socket.dispose(),
  };
}
