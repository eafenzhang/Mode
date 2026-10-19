import assert from "node:assert/strict";
import test from "node:test";
import type { AddressInfo } from "node:net";
import { WebSocket } from "ws";
import {
  ChannelClient,
  SocketProtocol,
  ProxyChannel,
  VSBuffer,
  Emitter,
  type ISocket,
} from "@mode/rpc";
import { MODE_RPC_HOST_CAPABILITY_HEADER } from "@mode/shared";
import type { ServiceCollection } from "@mode/services";
import { createHttpServer } from "../src/http.js";

// 局域网服务端「最近在本机打开的目录」录制：
// /ws/host 连接由 cookie 令牌解析出 clientId，该连接上任何携带 workspacePath 的 RPC
// （call/listen）回调 onLanClientWorkspace；纯 path 的目录浏览（readdir/resolvePath）不录。
// 失效模式（未实现时）：录制回调从不触发，断言失败。

const TOKEN = "lan-ws-recording-token";
const CLIENT_ID = "lan-client-uuid-1";

function wrapClientSocket(ws: WebSocket): ISocket {
  const onData = new Emitter<VSBuffer>();
  const onClose = new Emitter<void>();
  const onEnd = new Emitter<void>();
  // 与 lanRemoteAttach 相同的时序教训：监听必须在等待 open 之前挂上，
  // 否则服务端的 Initialize 帧会被丢弃，RPC 永远排队。
  ws.on("message", (data: unknown, isBinary: boolean) => {
    if (!isBinary) {
      return;
    }
    const bytes = Buffer.isBuffer(data) ? data : Buffer.from(data as ArrayBuffer);
    onData.fire(VSBuffer.wrap(new Uint8Array(bytes)));
  });
  ws.on("close", () => {
    onClose.fire();
    onEnd.fire();
  });
  ws.on("error", () => {
    onClose.fire();
    onEnd.fire();
  });
  return {
    onData: onData.event,
    onClose: onClose.event,
    onEnd: onEnd.event,
    write(buffer: VSBuffer) {
      if (ws.readyState === WebSocket.OPEN) {
        ws.send(buffer.buffer, { binary: true });
      }
    },
    end: () => ws.close(),
    drain: () => Promise.resolve(),
    dispose: () => ws.close(),
  };
}

function withTimeout<T>(promise: Promise<T>, ms: number, label: string): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`${label} 超时（${ms}ms）`)), ms);
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

test("携带 workspacePath 的 RPC 录制到该连接的 clientId；纯 path 浏览不录", async () => {
  const recorded: Array<{ clientId: string; path: string }> = [];
  const servicesStub = {
    getOptional: () => undefined,
    exposeOnChannelServer: (server: {
      registerChannel: (name: string, channel: unknown) => void;
    }) => {
      server.registerChannel(
        "mode-task",
        ProxyChannel.fromService({
          list: async (_arg: unknown) => [],
          readdir: async (_arg: unknown) => [],
        }),
      );
    },
  } as unknown as ServiceCollection;

  const server = createHttpServer(servicesStub, 0, {
    authToken: TOKEN,
    verifyAuthToken: (presented) => presented === TOKEN,
    resolveLanClientByToken: (presented) => (presented === TOKEN ? CLIENT_ID : null),
    onLanClientWorkspace: (clientId, workspacePath) => {
      recorded.push({ clientId, path: workspacePath });
    },
    serverId: "lan-ws-recording",
    name: "lan-ws-recording",
  });

  let ws: WebSocket | undefined;
  try {
    await new Promise<void>((resolve) => server.once("listening", () => resolve()));
    const port = (server.address() as AddressInfo).port;

    const capabilityResponse = await fetch(`http://127.0.0.1:${port}/api/rpc-host-capability`, {
      method: "POST",
      headers: { cookie: `mode_lite_token=${encodeURIComponent(TOKEN)}` },
    });
    assert.equal(capabilityResponse.status, 200);
    const { capability } = (await capabilityResponse.json()) as { capability?: string };
    assert.ok(capability, "capability 缺失");

    ws = new WebSocket(`ws://127.0.0.1:${port}/ws/host`, {
      headers: {
        [MODE_RPC_HOST_CAPABILITY_HEADER]: capability,
        cookie: `mode_lite_token=${encodeURIComponent(TOKEN)}`,
      },
    });
    const socket = wrapClientSocket(ws);
    const protocol = new SocketProtocol(socket);
    const client = new ChannelClient(protocol);
    await new Promise<void>((resolve, reject) => {
      ws?.once("open", () => resolve());
      ws?.once("error", reject);
    });

    const channel = client.getChannel<{ call: (name: string, arg?: unknown) => Promise<unknown> }>(
      "mode-task",
    );
    await withTimeout(channel.call("list", { workspacePath: "C:\\proj\\demo" }), 5_000, "list");
    await withTimeout(channel.call("readdir", { path: "C:\\proj" }), 5_000, "readdir");
    // 事件订阅同样携带 workspacePath（任务列表按 workspace 订阅）也要录。
    assert.deepEqual(recorded, [{ clientId: CLIENT_ID, path: "C:\\proj\\demo" }]);
  } finally {
    ws?.close();
    (server as unknown as { closeAllConnections?: () => void }).closeAllConnections?.();
    await new Promise<void>((resolve) => server.close(() => resolve()));
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
});
