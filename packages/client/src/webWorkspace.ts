import type { WebControlLink } from "@mode/remote-core/web";
import { SocketProtocol, type ChannelClient } from "@mode/rpc";
import type { IServiceAccessor } from "@mode/services";

import { connectViaProtocolWithClient } from "./websocket.js";
import { createWebWorkspaceSocket } from "./webWorkspaceSocket.js";

export interface WebWorkspaceConnection {
  services: IServiceAccessor;
  client: ChannelClient;
  dispose(): void;
}

/**
 * 浏览器通过 v3 E2EE 隧道挂接 Host 的 `/ws/host`，复用同一 SocketProtocol /
 * ChannelClient / RemoteServiceAccess。
 *
 * 传输断开时保持 services 引用不变：清分片、拒绝在途 RPC，后台重开隧道；
 * 新 ChannelServer 的 Initialize 触发 ChannelClient 重放事件订阅（与桌面挂接同构）。
 */
export async function openWebWorkspace(params: {
  link: Pick<WebControlLink, "request" | "openStream">;
  hostDeviceId: string;
  path?: string;
  timeoutMs?: number;
}): Promise<WebWorkspaceConnection> {
  let protocol: SocketProtocol | null = null;
  let client: ChannelClient | null = null;

  const workspace = createWebWorkspaceSocket({
    link: params.link,
    hostDeviceId: params.hostDeviceId,
    ...(params.path ? { path: params.path } : {}),
    ...(params.timeoutMs ? { timeoutMs: params.timeoutMs } : {}),
    onTransportLost: () => {
      // 旧连接可能停在半帧；清掉分片缓冲，并让在途 RPC 立即失败。
      protocol?.reset();
      client?.notifyTransportClosed(new Error("远程工作区隧道重连中，请重试"));
    },
    onFatal: () => {
      client?.dispose(new Error("远程工作区隧道已断开"));
    },
  });

  protocol = new SocketProtocol(workspace.socket);
  const connected = connectViaProtocolWithClient(protocol);
  client = connected.client;
  try {
    await workspace.start();
  } catch (error) {
    workspace.dispose();
    throw error;
  }
  return {
    services: connected.accessor,
    client: connected.client,
    dispose() {
      workspace.dispose();
    },
  };
}
