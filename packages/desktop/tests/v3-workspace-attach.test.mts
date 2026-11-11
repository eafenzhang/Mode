/**
 * v3 远程工作区 attachment：隧道 → services → 注册 → 释放。
 *
 * 覆盖 docs/specs/remote-control-v3-web-workspace.md §4/§6：
 * 1. 注册拿到的 services 来自 `/ws/host` 隧道，能完成 RPC；
 * 2. 注册失败必须回收隧道，不能留下未登记的连接。
 */
import assert from "node:assert/strict";
import test from "node:test";

import { attachV3WorkspaceSession, type V3WorkspaceSession } from "@mode/client";
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

interface Harness {
  link: Pick<WebControlLink, "request" | "openStream">;
  closed: number;
  openedPaths: string[];
}

function createHarness(): Harness {
  const harness: Harness = {
    closed: 0,
    openedPaths: [],
    link: {
      async request(_method: string, params: Record<string, unknown>) {
        assert.equal(params.path, "/api/rpc-host-capability");
        return {
          status: 200,
          headers: {},
          bodyBase64: base64(JSON.stringify({ capability: "cap-v3" })),
        };
      },
      openStream(_service: string, metadata: Record<string, unknown>): WebBusinessStream {
        harness.openedPaths.push(String(metadata.path));
        const [clientProtocol, serverProtocol] = createQueuePair();
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
            info: async () => ({ homedir: "C:\\v3-workspace", platform: "web" }),
          }),
        );
        setTimeout(() => server.ready(), 10);
        return createStream(clientProtocol, () => {
          harness.closed += 1;
        });
      },
    },
  };
  return harness;
}

function createStream(protocol: IMessagePassingProtocol, onClose: () => void): WebBusinessStream {
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
      onClose();
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

test("v3 工作区 attachment：注册后 services 可用，dispose 关闭隧道", async () => {
  const harness = createHarness();
  const registered: V3WorkspaceSession[] = [];
  const session = await attachV3WorkspaceSession({
    link: harness.link,
    hostDeviceId: "host-1",
    sessionId: "session-1",
    register: (value) => registered.push(value),
  });
  assert.equal(session.sessionId, "session-1");
  assert.deepEqual(harness.openedPaths, ["/ws/host"]);
  assert.equal(registered.length, 1);

  const info = await session.services.systemService.info();
  assert.equal(info.homedir, "C:\\v3-workspace");

  session.dispose();
  assert.equal(harness.closed, 1, "dispose 必须关闭隧道");
});

test("v3 工作区 attachment：注册失败时回收隧道并抛出", async () => {
  const harness = createHarness();
  await assert.rejects(
    attachV3WorkspaceSession({
      link: harness.link,
      hostDeviceId: "host-1",
      register: () => {
        throw new Error("registry rejected");
      },
    }),
    /registry rejected/u,
  );
  assert.equal(harness.closed, 1, "注册失败不能留下未登记的连接");
});
