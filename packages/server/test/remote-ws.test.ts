/**
 * `/ws/remote/:id` 的浏览器侧服务面（docs/specs/web-remote-workspace.md）。
 *
 * 用 fake LAN connection 起真实 HTTP/WS 服务，浏览器用 @mode/client 连入后调用
 * fileService，验证「完整 service collection → Initialize → RPC」整条链路。
 */
import assert from "node:assert/strict";
import type { AddressInfo } from "node:net";
import test from "node:test";

import { connectViaWebSocket } from "@mode/client";
import { ServiceCollection, type IServiceAccessor } from "@mode/services";
import type { LanPeerController } from "../src/remote/lanPeerController.js";
import { createHttpServer } from "../src/http.js";

function createFakeAccessor(): IServiceAccessor {
  const fileService = {
    readdir: async () => [{ name: "hello.txt", path: "/hello.txt" }],
  };
  // 每个必需服务给一个空对象；createRemoteServiceCollection 只做注册，
  // 本用例只调用 fileService，足以证明多服务 channel 注册成功。
  return new Proxy(
    { fileService },
    {
      get: (target, property) =>
        property === "fileService" ? target.fileService : ({} as Record<string, never>),
    },
  ) as unknown as IServiceAccessor;
}

test("浏览器连入 /ws/remote/:id 后可调用远端 fileService", async () => {
  const accessor = createFakeAccessor();
  const disposed: string[] = [];
  const fakeController = {
    takeRemoteConnection: (id: string) =>
      id === "session-1"
        ? {
            services: accessor,
            dispose: async () => {
              disposed.push(id);
            },
          }
        : null,
  } as unknown as LanPeerController;

  const server = createHttpServer({} as unknown as ServiceCollection, 0, {
    authToken: "test-token",
    authRequired: true,
    lanPeers: fakeController,
  });
  await new Promise<void>((resolve) => server.once("listening", () => resolve()));
  const port = (server.address() as AddressInfo).port;

  const sockets: WebSocket[] = [];
  try {
    const services = await connectViaWebSocket(
      `ws://127.0.0.1:${String(port)}/ws/remote/session-1?token=test-token`,
      { onOpenSocket: (ws) => sockets.push(ws) },
    );
    const entries = await services.fileService.readdir({ path: "/" });
    assert.deepEqual(entries, [{ name: "hello.txt", path: "/hello.txt" }]);
  } finally {
    sockets[0]?.close();
    await new Promise<void>((resolve) => server.close(() => resolve()));
    await new Promise<void>((resolve) => setTimeout(resolve, 50));
  }
  assert.deepEqual(disposed, ["session-1"]);
});
