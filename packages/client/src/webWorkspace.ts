import type { WebBusinessStream, WebControlLink } from "@mode/remote-core/web";
import {
  Emitter,
  SocketProtocol,
  VSBuffer,
  type ChannelClient,
  type ISocket,
} from "@mode/rpc";
import type { IServiceAccessor } from "@mode/services";

import { connectViaProtocolWithClient } from "./websocket.js";

export interface WebWorkspaceConnection {
  services: IServiceAccessor;
  client: ChannelClient;
  dispose(): void;
}

function decodeBase64(bodyBase64: string): Uint8Array {
  const binary = atob(bodyBase64);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes;
}

function createStreamSocket(stream: WebBusinessStream): ISocket {
  const onData = new Emitter<VSBuffer>({
    onWillAddFirstListener: () => {
      attached = true;
      for (const bytes of pending.splice(0, pending.length)) onData.fire(VSBuffer.wrap(bytes));
    },
  });
  const onClose = new Emitter<void>();
  const onEnd = new Emitter<void>();
  const pending: Uint8Array[] = [];
  let attached = false;
  let closed = false;
  const emitClose = (): void => {
    if (closed) return;
    closed = true;
    onClose.fire();
    onEnd.fire();
  };

  stream.onData((frame) => {
    if (frame.length === 0 || frame[0] !== 1) return;
    const payload = frame.subarray(1);
    if (attached) onData.fire(VSBuffer.wrap(payload));
    else pending.push(payload);
  });
  stream.onClose(() => emitClose());

  return {
    onData: onData.event,
    onClose: onClose.event,
    onEnd: onEnd.event,
    write(buffer: VSBuffer) {
      if (closed) return;
      const frame = new Uint8Array(buffer.byteLength + 1);
      frame[0] = 1;
      frame.set(buffer.buffer.subarray(0, buffer.byteLength), 1);
      try {
        stream.send(frame);
      } catch {
        emitClose();
      }
    },
    end() {
      stream.close("client closed");
    },
    drain() {
      return Promise.resolve();
    },
    dispose() {
      stream.close("client disposed");
      emitClose();
    },
  };
}

/**
 * 浏览器通过 v3 E2EE 隧道挂接 Host 的 `/ws/host`，复用同一 SocketProtocol /
 * ChannelClient / RemoteServiceAccess。Host 本机 LAN 服务未启用时 capability 请求失败。
 */
export async function openWebWorkspace(params: {
  link: Pick<WebControlLink, "request" | "openStream">;
  hostDeviceId: string;
  path?: string;
  timeoutMs?: number;
}): Promise<WebWorkspaceConnection> {
  const timeoutMs = params.timeoutMs ?? 15_000;
  const response = (await params.link.request(
    "remote.http.request",
    { method: "POST", path: "/api/rpc-host-capability" },
    timeoutMs,
  )) as { status?: unknown; bodyBase64?: unknown };
  if (response.status !== 200 || typeof response.bodyBase64 !== "string") {
    throw new Error(`申请对端连接票据失败（HTTP ${String(response.status)}）`);
  }
  const capabilityBody = JSON.parse(new TextDecoder().decode(decodeBase64(response.bodyBase64))) as {
    capability?: unknown;
  };
  if (typeof capabilityBody.capability !== "string" || !capabilityBody.capability) {
    throw new Error("对端未返回连接票据");
  }

  const stream = params.link.openStream("remote.ws", {
    path: params.path ?? "/ws/host",
    headers: { "x-mode-rpc-host-capability": capabilityBody.capability },
  });
  const socket = createStreamSocket(stream);
  const protocol = new SocketProtocol(socket);
  const connected = connectViaProtocolWithClient(protocol);
  socket.onClose(() => {
    connected.client.dispose(new Error("远程工作区隧道已关闭"));
  });
  return {
    services: connected.accessor,
    client: connected.client,
    dispose() {
      socket.dispose();
    },
  };
}
