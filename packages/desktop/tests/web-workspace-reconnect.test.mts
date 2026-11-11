/**
 * Web 工作区隧道级重连：断开后同一 services 引用继续完成 RPC。
 *
 * 覆盖 docs/specs/remote-control-v3-web.md 的「隧道级重连」：传输断开时
 * `SocketProtocol.reset` + `ChannelClient.notifyTransportClosed`，重开 `/ws/host`
 * 后由新 ChannelServer 的 Initialize 重放事件订阅。
 */
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

async function waitFor(predicate: () => boolean, what: string): Promise<void> {
  const deadline = Date.now() + 5_000;
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error(`等待 ${what} 超时`);
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
}

test("Web 工作区隧道断开后自动重开，同一 services 继续 RPC", async () => {
  const streams: { stream: WebBusinessStream; closeFromPeer(): void }[] = [];
  let sessions = 0;
  const link = {
    async request() {
      return {
        status: 200,
        headers: {},
        bodyBase64: base64(JSON.stringify({ capability: "cap-reconnect" })),
      };
    },
    openStream(_service: string, metadata: Record<string, unknown>): WebBusinessStream {
      assert.equal(metadata.path, "/ws/host");
      sessions += 1;
      const session = sessions;
      const [clientProtocol, serverProtocol] = createQueuePair();
      const closeListeners = new Set<(reason: string) => void>();
      const serverSocket = {
        onData: serverProtocol.onMessage,
        onClose: new Emitter<void>().event,
        onEnd: new Emitter<void>().event,
        write: (buffer: VSBuffer) => serverProtocol.send(buffer),
        end: () => undefined,
        drain: () => Promise.resolve(),
        dispose: () => undefined,
      } as unknown as ISocket;
      const server = new ChannelServer(new SocketProtocol(serverSocket), `ctx-${session}`, 1_000, true);
      server.registerChannel(
        ISystemService.channelName,
        ProxyChannel.fromService({
          info: async () => ({ homedir: `C:\\workspace-${session}`, platform: "web" }),
        }),
      );
      setTimeout(() => server.ready(), 10);
      const stream = createStream(clientProtocol, () => {
        for (const listener of closeListeners) listener("transport closed");
        closeListeners.clear();
      });
      streams.push({
        stream,
        closeFromPeer: () => {
          for (const listener of closeListeners) listener("transport closed");
          closeListeners.clear();
        },
      });
      // 记录 onClose 订阅，供测试模拟对端断开。
      const originalOnClose = stream.onClose.bind(stream);
      stream.onClose = (callback) => {
        closeListeners.add(callback);
        return originalOnClose(callback);
      };
      return stream;
    },
  } as Pick<WebControlLink, "request" | "openStream">;

  const workspace = await openWebWorkspace({ link, hostDeviceId: "host-1" });
  try {
    const first = await workspace.services.systemService.info();
    assert.equal(first.homedir, "C:\\workspace-1");

    streams[0]!.closeFromPeer();
    await waitFor(() => streams.length >= 2, "重开第二条隧道");
    await waitFor(() => sessions >= 2, "新 ChannelServer");

    const second = await workspace.services.systemService.info();
    assert.equal(second.homedir, "C:\\workspace-2", "重连后请求落到新会话");
  } finally {
    workspace.dispose();
  }
});

function createStream(protocol: IMessagePassingProtocol, onPeerClose: () => void): WebBusinessStream {
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
      onPeerClose();
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
