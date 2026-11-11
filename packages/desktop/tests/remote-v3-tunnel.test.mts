import assert from "node:assert/strict";
import test from "node:test";

import type { RemoteStream } from "@mode/remote-core";
import type { RemoteV3TunnelEvent } from "@mode/shared";

import { RemoteV3TunnelRuntime } from "../src/host/remoteV3/tunnels.js";

function createFakeStream(): {
  stream: RemoteStream;
  deliver(data: Uint8Array): void;
  close(reason: string): void;
} {
  const dataListeners = new Set<(data: Uint8Array) => void>();
  const closeListeners = new Set<(reason: string) => void>();
  return {
    stream: {
      streamId: 1,
      service: "remote.ws",
      metadata: {},
      send: () => undefined,
      close: () => undefined,
      onData: (callback) => {
        dataListeners.add(callback);
        return () => dataListeners.delete(callback);
      },
      onClose: (callback) => {
        closeListeners.add(callback);
        return () => closeListeners.delete(callback);
      },
    },
    deliver: (data) => {
      for (const listener of dataListeners) listener(data);
    },
    close: (reason) => {
      for (const listener of closeListeners) listener(reason);
    },
  };
}

test("隧道在订阅前产生的 open/message 事件会在首个订阅者补齐", async () => {
  const fake = createFakeStream();
  const runtime = new RemoteV3TunnelRuntime(async () => fake.stream);
  const { tunnelId } = await runtime.open({
    hostDeviceId: "host-1",
    path: "/ws/host",
  });
  // 模拟渲染进程尚未订阅 onTunnelEvent 时对端已经发来首帧。
  fake.deliver(new Uint8Array([1, 2, 3]));

  const events: RemoteV3TunnelEvent[] = [];
  const subscription = runtime.onEvent((event) => events.push(event));
  assert.deepEqual(
    events.map((event) => event.kind),
    ["open", "message"],
  );
  assert.equal(events[0]?.tunnelId, tunnelId);
  assert.equal(events[1]?.tunnelId, tunnelId);
  subscription.dispose();
});
