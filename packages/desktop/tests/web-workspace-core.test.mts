import assert from "node:assert/strict";
import test from "node:test";

import { openWebWorkspace } from "@mode/client";
import {
  ChannelServer,
  Emitter,
  ProxyChannel,
  SocketProtocol,
  VSBuffer,
  createQueuePair,
  type IMessagePassingProtocol,
  type ISocket,
} from "@mode/rpc";
import { ISystemService } from "@mode/services";
import type { WebBusinessStream, WebControlLink } from "@mode/remote-core/web";

function base64(text: string): string {
  return Buffer.from(text, "utf8").toString("base64");
}

test("Web 工作区通过 remote.ws 隧道复用 SocketProtocol/ChannelClient", async () => {
  let requestedPath = "";
  let requestedHeader: unknown = null;
  const link = {
    async request(_method: string, params: Record<string, unknown>) {
      requestedPath = String(params.path);
      return {
        status: 200,
        headers: {},
        bodyBase64: base64(JSON.stringify({ capability: "cap-1" })),
      };
    },
    openStream(_service: string, metadata: Record<string, unknown>): WebBusinessStream {
      const headers = metadata.headers as Record<string, unknown> | undefined;
      requestedHeader = headers?.["x-mode-rpc-host-capability"];
      const [clientProtocol, serverProtocol] = createQueuePair();
      // ChannelServer 依赖 SocketProtocol 的 13 字节帧头；测试里也要补 server 侧包装。
      const serverSocket = {
        onData: serverProtocol.onMessage,
        onClose: new Emitter<void>().event,
        onEnd: new Emitter<void>().event,
        write: (buffer: VSBuffer) => serverProtocol.send(buffer),
        end: () => undefined,
        drain: () => Promise.resolve(),
        dispose: () => undefined,
      } as unknown as ISocket;
      const server = new ChannelServer(new SocketProtocol(serverSocket), "ctx", 1_000, true);
      server.registerChannel(
        ISystemService.channelName,
        ProxyChannel.fromService({
          info: async () => ({ homedir: "C:\\web-workspace", platform: "web" }),
        }),
      );
      setTimeout(() => server.ready(), 10);
      return createStream(clientProtocol);
    },
  } as Pick<WebControlLink, "request" | "openStream">;

  const connection = await openWebWorkspace({ link, hostDeviceId: "host-1" });
  try {
    const info = await connection.services.systemService.info();
    assert.equal(info.homedir, "C:\\web-workspace");
    assert.equal(requestedPath, "/api/rpc-host-capability");
    assert.equal(requestedHeader, "cap-1");
  } finally {
    connection.dispose();
  }
});

function createStream(protocol: IMessagePassingProtocol): WebBusinessStream {
  const dataListeners = new Set<(bytes: Uint8Array) => void>();
  const closeListeners = new Set<(reason: string) => void>();
  protocol.onMessage((buffer) => {
    const payload = new Uint8Array(buffer.byteLength + 1);
    payload[0] = 1;
    payload.set(buffer.buffer.subarray(0, buffer.byteLength), 1);
    for (const listener of dataListeners) listener(payload);
  });
  return {
    streamId: 1,
    send(frame) {
      protocol.send(VSBuffer.wrap(frame.subarray(1)));
    },
    close(reason = "closed") {
      for (const listener of closeListeners) listener(reason);
      closeListeners.clear();
    },
    onData(callback) {
      dataListeners.add(callback);
      return () => dataListeners.delete(callback);
    },
    onClose(callback) {
      closeListeners.add(callback);
      return () => closeListeners.delete(callback);
    },
  };
}
