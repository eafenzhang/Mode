/**
 * LR-4：`@mode/rpc` 的帧多路复用原语单测（生产实现，不是 spike 里的内联原型）。
 *
 * 守：base 帧逐字节不变、scoped 帧编解码、未知/畸形帧丢弃计数、重复 attach 明确失败、
 * 两个 scope 并发不串台。
 */
import assert from "node:assert/strict";
import test from "node:test";

import {
  ChannelClient,
  ChannelServer,
  MAX_SCOPE_ID,
  VSBuffer,
  createFrameDemultiplexer,
  decodeFrame,
  encodeScopedFrame,
  type IChannel,
  type IMessagePassingProtocol,
  type IServerChannel,
} from "@mode/rpc";

function transportPair(): {
  a: IMessagePassingProtocol;
  b: IMessagePassingProtocol;
} {
  const aListeners = new Set<(buffer: VSBuffer) => void>();
  const bListeners = new Set<(buffer: VSBuffer) => void>();
  const a: IMessagePassingProtocol = {
    send: (buffer) => {
      for (const listener of bListeners) listener(buffer);
    },
    onMessage: (listener) => {
      aListeners.add(listener);
      return { dispose: () => aListeners.delete(listener) };
    },
  };
  const b: IMessagePassingProtocol = {
    send: (buffer) => {
      for (const listener of aListeners) listener(buffer);
    },
    onMessage: (listener) => {
      bListeners.add(listener);
      return { dispose: () => bListeners.delete(listener) };
    },
  };
  return { a, b };
}

function echoChannel(label: string): IServerChannel {
  return {
    async call(_ctx, command, arg) {
      if (command === "who") return label;
      if (command === "echo") return arg;
      throw new Error(`unknown command ${command}`);
    },
    listen(_ctx, event) {
      throw new Error(`unknown event ${event}`);
    },
  };
}

test("编解码：base 帧原样、scoped 帧带 id、畸形 marker 返回 null", () => {
  const base = Uint8Array.of(100, 1, 2, 3);
  const decodedBase = decodeFrame(base);
  assert.equal(decodedBase?.scopeId, null);
  assert.equal(decodedBase?.payload, base, "base payload 不复制");

  const payload = Uint8Array.of(200, 9);
  const scoped = encodeScopedFrame(7, payload);
  const decodedScoped = decodeFrame(scoped);
  assert.equal(decodedScoped?.scopeId, 7);
  assert.deepEqual([...(decodedScoped?.payload ?? [])], [200, 9]);

  assert.equal(decodeFrame(Uint8Array.of(0xff, 0x00)), null, "不足 5 字节的 marker 帧必须丢弃");
  assert.throws(() => encodeScopedFrame(0, payload), /scope id/u);
  assert.throws(() => encodeScopedFrame(MAX_SCOPE_ID + 1, payload), /scope id/u);
  assert.throws(() => encodeScopedFrame(1.5, payload), /scope id/u);
});

test("两个 scope 并发复用：base/scoped 不串台，未知与畸形帧只计数", async () => {
  const { a, b } = transportPair();
  // a = 页面侧，b = Host 侧；两个 demux 各自解复用。
  const pageDemux = createFrameDemultiplexer(a);
  const hostDemux = createFrameDemultiplexer(b);

  const scopeOne: IServerChannel = echoChannel("one");
  const scopeTwo: IServerChannel = echoChannel("two");

  const baseClient = new ChannelClient(pageDemux.base);
  const baseServer = new ChannelServer(hostDemux.base, "base", 1_000, true);
  baseServer.registerChannel("echo", echoChannel("base"));
  baseServer.ready();

  const oneClient = new ChannelClient(pageDemux.attachScope(1));
  const oneServer = new ChannelServer(hostDemux.attachScope(1), "one", 1_000, true);
  oneServer.registerChannel("echo", scopeOne);
  oneServer.ready();

  const twoClient = new ChannelClient(pageDemux.attachScope(2));
  const twoServer = new ChannelServer(hostDemux.attachScope(2), "two", 1_000, true);
  twoServer.registerChannel("echo", scopeTwo);
  twoServer.ready();

  const who = (client: ChannelClient): Promise<string> =>
    client.getChannel<IChannel>("echo").call<string>("who");
  assert.deepEqual(await Promise.all([who(baseClient), who(oneClient), who(twoClient)]), [
    "base",
    "one",
    "two",
  ]);

  // 未知 scope：Host 侧没有 9，必须丢弃并计数。
  a.send(VSBuffer.wrap(encodeScopedFrame(9, Uint8Array.of(100, 1, 2))));
  assert.equal(hostDemux.unknownScopeFrames, 1);

  // 畸形 marker 帧：不能进 base，也不能误投 scope。
  a.send(VSBuffer.wrap(Uint8Array.of(0xff, 0x00)));
  assert.equal(hostDemux.invalidFrames, 1);

  // detach 后同一 id 的帧按未知 scope 计数。
  hostDemux.detachScope(1);
  pageDemux.detachScope(1);
  a.send(VSBuffer.wrap(encodeScopedFrame(1, Uint8Array.of(100, 1, 2))));
  assert.equal(hostDemux.unknownScopeFrames, 2);

  // 另一个 scope 与 base 继续工作。
  assert.equal(await who(twoClient), "two");
  assert.equal(await who(baseClient), "base");
});

test("重复 attach 同一 scope id 是配置错误", () => {
  const { a } = transportPair();
  const demux = createFrameDemultiplexer(a);
  demux.attachScope(1);
  assert.throws(() => demux.attachScope(1), /已经绑定/u);
  demux.dispose();
});

test("dispose 后不再接收任何帧", () => {
  const { a } = transportPair();
  const demux = createFrameDemultiplexer(a);
  const baseEvents: unknown[] = [];
  demux.base.onMessage((buffer) => baseEvents.push(buffer));
  demux.dispose();
  a.send(VSBuffer.wrap(Uint8Array.of(100, 1, 2)));
  assert.equal(baseEvents.length, 0);
});
