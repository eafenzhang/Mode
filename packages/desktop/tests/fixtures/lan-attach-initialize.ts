/**
 * LAN attach Initialize 回归夹具：走真实全链路（createHttpServer → attachLanRemoteConnection →
 * 首条 RPC），钉住「message 监听必须先于 await open 挂载」这一时序。
 *
 * 服务端 ChannelServer 只在建连瞬间发一帧 Initialize；若客户端监听晚挂把它丢掉，
 * ChannelClient 永久停在 Uninitialized，system.info 等所有 RPC 静默排队——
 * 本夹具以 4s 超时把它变成显式失败。成功输出 LAN_ATTACH_RPC_OK。
 */
import type { AddressInfo } from "node:net";
import { createHttpServer } from "@mode/server";
import { ProxyChannel, type IChannelServer } from "@mode/rpc";
import { ISystemService, type ServiceCollection } from "@mode/services";
import {
  attachLanRemoteConnection,
  type LanRemoteConnection,
} from "../../src/host/lanRemoteAttach.js";

const TOKEN = "lan-test-token";
const RPC_TIMEOUT_MS = 4_000;

// createHttpServer 只用到 getOptional 与 exposeOnChannelServer；
// 系统服务用最简桩即可——本夹具断言的是握手时序，不是业务实现。
const servicesStub = {
  getOptional: () => undefined,
  exposeOnChannelServer: (server: IChannelServer) => {
    server.registerChannel(
      ISystemService.channelName,
      ProxyChannel.fromService({
        info: async () => ({ homedir: "C:\\lan-fixture", platform: "win32" }),
      }),
    );
  },
} as unknown as ServiceCollection;

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
  serverId: "lan-fixture",
  name: "lan-fixture",
});

let connection: LanRemoteConnection | undefined;
try {
  await new Promise<void>((resolve) => server.once("listening", () => resolve()));
  const port = (server.address() as AddressInfo).port;

  connection = await attachLanRemoteConnection({
    host: "127.0.0.1",
    port,
    token: TOKEN,
  });
  const info = await withTimeout(
    connection.services.systemService.info(),
    RPC_TIMEOUT_MS,
    "systemService.info()",
  );
  if (info.homedir !== "C:\\lan-fixture") {
    throw new Error(`RPC 返回了非夹具数据：${JSON.stringify(info)}`);
  }
  console.log("LAN_ATTACH_RPC_OK");
} finally {
  // 清理必须收干净句柄并让进程自然退出：ws 关闭握手进行中就 process.exit
  // 会在 Windows libuv 上触发 UV_HANDLE_CLOSING 断言（实测 E3/E4），
  // 那是退出路径噪声，与被测握手行为无关。
  await connection?.dispose().catch(() => undefined);
  (server as unknown as { closeAllConnections?: () => void }).closeAllConnections?.();
  await new Promise<void>((resolve) => server.close(() => resolve()));
  await new Promise((resolve) => setTimeout(resolve, 300));
}
