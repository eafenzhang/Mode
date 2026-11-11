/**
 * v3 远程工作区挂接回归夹具：真实 HTTP/WS 服务端 + 假 v3 控制面。
 *
 * 覆盖 `attachV3RemoteWorkspace` 的完整链路：
 * `/api/server-info` → `/api/rpc-host-capability` → `/ws/host` 隧道 →
 * `SocketProtocol → ChannelClient` 首条 RPC。
 *
 * 与 LAN attach 一样，服务端建连即发 `Initialize`；隧道包装必须在此之前挂好监听，
 * 否则 ChannelClient 永久停在 Uninitialized。成功输出 V3_WORKSPACE_RPC_OK。
 */
import type { AddressInfo } from "node:net";
import { WebSocket } from "ws";
import { ProxyChannel, type IChannelServer } from "@mode/rpc";
import { decodeBase64Url, encodeBase64Url } from "@mode/remote-core";
import { createHttpServer } from "@mode/server";
import { ISystemService, type IRemoteV3Service, type ServiceCollection } from "@mode/services";
import {
  MODE_RPC_HOST_CAPABILITY_HEADER,
  type RemoteV3TunnelEvent,
  type RemoteV3TunnelOpenParams,
} from "@mode/shared";
import { attachV3RemoteWorkspace } from "../../src/host/remoteV3/workspaceAttach.js";

const TOKEN = "v3-workspace-test-token";
const RPC_TIMEOUT_MS = 4_000;

const servicesStub = {
  getOptional: () => undefined,
  exposeOnChannelServer: (server: IChannelServer) => {
    server.registerChannel(
      ISystemService.channelName,
      ProxyChannel.fromService({
        info: async () => ({ homedir: "C:\\v3-fixture", platform: "win32" }),
      }),
    );
  },
} as unknown as ServiceCollection;

async function waitForAsync(predicate: () => boolean, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error("等待隧道重连超时");
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

function withTimeout<T>(promise: Promise<T>, ms: number, label: string): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error(`${label} 在 ${ms}ms 内未完成（Initialize 大概率被丢弃，RPC 悬空）`)),
      ms,
    );
    promise.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error: unknown) => {
        clearTimeout(timer);
        reject(error instanceof Error ? error : new Error(String(error)));
      },
    );
  });
}

const server = createHttpServer(servicesStub, 0, {
  authToken: TOKEN,
  verifyAuthToken: (presented) => presented === TOKEN,
  serverId: "v3-fixture",
  name: "v3-fixture",
});

const listeners = new Set<(event: RemoteV3TunnelEvent) => void>();
const tunnels = new Map<string, WebSocket>();
let nextTunnelId = 1;
let openTunnelCount = 0;

function emit(event: RemoteV3TunnelEvent): void {
  for (const listener of listeners) listener(event);
}

function toLocalUrl(path: string): string {
  const base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  return new URL(path, base).toString();
}

const fakeRemoteV3 = {
  async httpRequest(params: { method: string; path: string; headers?: Record<string, string> }) {
    const response = await fetch(toLocalUrl(params.path), {
      method: params.method,
      headers: {
        cookie: `mode_lite_token=${TOKEN}`,
        ...params.headers,
      },
    });
    return {
      status: response.status,
      headers: Object.fromEntries(response.headers.entries()),
      bodyBase64: Buffer.from(await response.arrayBuffer()).toString("base64"),
    };
  },
  async openTunnel(params: RemoteV3TunnelOpenParams) {
    if (!params.headers?.[MODE_RPC_HOST_CAPABILITY_HEADER]) {
      throw new Error("缺少 host capability 头");
    }
    const url = new URL(toLocalUrl(params.path));
    url.protocol = "ws:";
    const socket = new WebSocket(url, {
      headers: {
        cookie: `mode_lite_token=${TOKEN}`,
        ...params.headers,
      },
    });
    const tunnelId = `tunnel-${nextTunnelId++}`;
    openTunnelCount += 1;
    tunnels.set(tunnelId, socket);
    socket.on("message", (data, isBinary) => {
      const payload = Buffer.isBuffer(data)
        ? data
        : Array.isArray(data)
          ? Buffer.concat(data)
          : Buffer.from(data as ArrayBuffer);
      emit({
        tunnelId,
        kind: "message",
        data: encodeBase64Url(Buffer.concat([Buffer.from([isBinary ? 1 : 0]), payload])),
      });
    });
    socket.on("close", () => {
      tunnels.delete(tunnelId);
      emit({ tunnelId, kind: "close", reason: "local ws closed" });
    });
    // 与真实 `RemoteV3TunnelRuntime` 一致：不等 WS open 就返回 tunnelId，
    // 让调用方在任何数据帧到达前完成事件订阅（Initialize 只在建连瞬间发一次）。
    emit({ tunnelId, kind: "open" });
    return { tunnelId };
  },
  async sendTunnel(tunnelId: string, data: string) {
    const socket = tunnels.get(tunnelId);
    if (!socket) throw new Error("隧道不存在或已关闭");
    const frame = decodeBase64Url(data, "tunnel.data");
    socket.send(Buffer.from(frame.subarray(1)), { binary: frame[0] === 1 });
  },
  async closeTunnel(tunnelId: string) {
    tunnels.get(tunnelId)?.close();
  },
  onTunnelEvent(listener: (event: RemoteV3TunnelEvent) => void) {
    listeners.add(listener);
    return {
      dispose: () => {
        listeners.delete(listener);
      },
    };
  },
} as unknown as IRemoteV3Service;

let connection: Awaited<ReturnType<typeof attachV3RemoteWorkspace>> | undefined;
try {
  await new Promise<void>((resolve) => server.once("listening", () => resolve()));

  connection = await attachV3RemoteWorkspace({
    remoteV3: fakeRemoteV3,
    hostDeviceId: "host-fixture",
  });
  if (connection.info.serverId !== "v3-fixture") {
    throw new Error(`server-info 未按隧道返回：${JSON.stringify(connection.info)}`);
  }
  const info = await withTimeout(
    connection.services.systemService.info(),
    RPC_TIMEOUT_MS,
    "systemService.info()",
  );
  if (info.homedir !== "C:\\v3-fixture") {
    throw new Error(`RPC 返回了非夹具数据：${JSON.stringify(info)}`);
  }
  // capability 头必须真的随隧道握手送达：缺失时服务端会拒绝升级。
  if (!tunnels.size) {
    throw new Error("隧道未建立");
  }

  // 模拟隧道瞬断：services 引用不变，重连后同一 ChannelClient 继续工作。
  const tunnelCountBeforeDrop = openTunnelCount;
  for (const socket of tunnels.values()) socket.close();
  await waitForAsync(() => openTunnelCount > tunnelCountBeforeDrop, 5_000);
  const infoAfterReconnect = await withTimeout(
    connection.services.systemService.info(),
    RPC_TIMEOUT_MS,
    "reconnected systemService.info()",
  );
  if (infoAfterReconnect.homedir !== "C:\\v3-fixture") {
    throw new Error(`重连后 RPC 返回了非夹具数据：${JSON.stringify(infoAfterReconnect)}`);
  }
  console.log("V3_WORKSPACE_RPC_OK");
} finally {
  await connection?.dispose().catch(() => undefined);
  (server as unknown as { closeAllConnections?: () => void }).closeAllConnections?.();
  await new Promise<void>((resolve) => server.close(() => resolve()));
  await new Promise((resolve) => setTimeout(resolve, 300));
}
