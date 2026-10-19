/**
 * LR-4 · DR-LR-2 spike：**单端口多路复用**的可执行验证。
 *
 * 目标不是生产实现，而是在不改 Rust 桥的前提下证明：
 * 1. base scope 的帧格式完全不变（现有的 Host 端口往返用例继续成立）；
 * 2. 同一根传输上可以并发承载两个 remote scope，响应不会串台；
 * 3. scope 关闭后只回收该 scope，base 与其它 scope 继续工作；未知 scope 的帧被丢弃；
 * 4. 大帧（256KB）经过前缀封装后仍完整。
 *
 * 结论写进 `docs/plans/2026-10-19-lr4-attachment-spike.md`；生产化要点见文档的迁移步骤。
 * 本文件内联实现原型，**不修改生产传输层**（spike 未验收前不接线）。
 */
import assert from "node:assert/strict";
import test from "node:test";

import {
  ChannelClient,
  ChannelServer,
  Emitter,
  VSBuffer,
  type IChannel,
  type IMessagePassingProtocol,
  type IServerChannel,
} from "@mode/rpc";

/** 保留首字节：RequestType 从 100 起、ResponseType 从 200 起，0xFF 不是合法帧类型。 */
const SCOPED_FRAME_MARKER = 0xff;
const HEADER_BYTES = 5;

function encodeScopedFrame(scopeId: number, frame: Uint8Array): Uint8Array {
  const out = new Uint8Array(HEADER_BYTES + frame.byteLength);
  out[0] = SCOPED_FRAME_MARKER;
  new DataView(out.buffer, out.byteOffset, out.byteLength).setUint32(1, scopeId, false);
  out.set(frame, HEADER_BYTES);
  return out;
}

function decodeFrame(frame: Uint8Array): { scopeId: number | null; payload: Uint8Array } {
  if (frame.byteLength >= HEADER_BYTES && frame[0] === SCOPED_FRAME_MARKER) {
    const view = new DataView(frame.buffer, frame.byteOffset, frame.byteLength);
    return { scopeId: view.getUint32(1, false), payload: frame.subarray(HEADER_BYTES) };
  }
  return { scopeId: null, payload: frame };
}

function createInMemoryTransportPair(): {
  client: IMessagePassingProtocol;
  server: IMessagePassingProtocol;
} {
  const clientListeners = new Set<(buffer: VSBuffer) => void>();
  const serverListeners = new Set<(buffer: VSBuffer) => void>();
  const client: IMessagePassingProtocol = {
    send: (buffer) => {
      for (const listener of serverListeners) listener(buffer);
    },
    onMessage: (listener) => {
      clientListeners.add(listener);
      return { dispose: () => clientListeners.delete(listener) };
    },
  };
  const server: IMessagePassingProtocol = {
    send: (buffer) => {
      for (const listener of clientListeners) listener(buffer);
    },
    onMessage: (listener) => {
      serverListeners.add(listener);
      return { dispose: () => serverListeners.delete(listener) };
    },
  };
  return { client, server };
}

interface Demux {
  base: IMessagePassingProtocol;
  createScope(scopeId: number): IMessagePassingProtocol;
  disposeScope(scopeId: number): void;
  droppedScopes: number[];
}

/** 一端的解复用：base 帧原样、带 marker 的帧按 scope id 投递。 */
function createDemux(transport: IMessagePassingProtocol): Demux {
  const baseEmitter = new Emitter<VSBuffer>();
  const scopeEmitters = new Map<number, Emitter<VSBuffer>>();
  const droppedScopes: number[] = [];
  transport.onMessage((buffer) => {
    const { scopeId, payload } = decodeFrame(buffer.buffer);
    if (scopeId === null) {
      baseEmitter.fire(VSBuffer.wrap(payload));
      return;
    }
    const emitter = scopeEmitters.get(scopeId);
    if (emitter) {
      emitter.fire(VSBuffer.wrap(payload));
    } else {
      droppedScopes.push(scopeId);
    }
  });
  return {
    base: { send: (buffer) => transport.send(buffer), onMessage: baseEmitter.event },
    createScope(scopeId: number): IMessagePassingProtocol {
      const emitter = new Emitter<VSBuffer>();
      scopeEmitters.set(scopeId, emitter);
      return {
        send: (buffer) => transport.send(VSBuffer.wrap(encodeScopedFrame(scopeId, buffer.buffer))),
        onMessage: emitter.event,
      };
    },
    disposeScope(scopeId: number): void {
      scopeEmitters.get(scopeId)?.dispose();
      scopeEmitters.delete(scopeId);
    },
    droppedScopes,
  };
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

/** 客户端端：先建（等 Initialize）。 */
function createClient(demux: Demux, scopeId: number | null): ChannelClient {
  const protocol = scopeId === null ? demux.base : demux.createScope(scopeId);
  return new ChannelClient(protocol);
}

/** 服务端端：后建（构造后 ready 发 Initialize）。 */
function createServer(demux: Demux, scopeId: number | null, label: string): ChannelServer {
  const protocol = scopeId === null ? demux.base : demux.createScope(scopeId);
  const server = new ChannelServer(protocol, label, 1_000, true);
  server.registerChannel("echo", echoChannel(label));
  server.ready();
  return server;
}

test("base 与两个 remote scope 并发复用一根传输，响应不串台", async () => {
  const transport = createInMemoryTransportPair();
  const serverDemux = createDemux(transport.server);
  const clientDemux = createDemux(transport.client);

  // 顺序是协议的一部分：客户端先订阅，服务端的 Initialize 才有人接。
  const baseClient = createClient(clientDemux, null);
  const scope1Client = createClient(clientDemux, 1);
  const scope2Client = createClient(clientDemux, 2);
  createServer(serverDemux, null, "base");
  createServer(serverDemux, 1, "scope-1");
  createServer(serverDemux, 2, "scope-2");

  const who = (client: ChannelClient): Promise<string> =>
    client.getChannel<IChannel>("echo").call<string>("who");
  assert.deepEqual(await Promise.all([who(baseClient), who(scope1Client), who(scope2Client)]), [
    "base",
    "scope-1",
    "scope-2",
  ]);

  // 大帧：前缀不能破坏序列化边界。
  const big = "x".repeat(256 * 1024);
  assert.equal(await scope1Client.getChannel<IChannel>("echo").call<string>("echo", big), big);
  assert.equal(
    await baseClient.getChannel<IChannel>("echo").call<string>("who"),
    "base",
    "大帧之后 base 仍然可用",
  );
});

test("scope 回收只影响自己；未知 scope 的帧被丢弃", async () => {
  const transport = createInMemoryTransportPair();
  const serverDemux = createDemux(transport.server);
  const clientDemux = createDemux(transport.client);
  const baseClient = createClient(clientDemux, null);
  const scope1Client = createClient(clientDemux, 1);
  const scope2Client = createClient(clientDemux, 2);
  createServer(serverDemux, null, "base");
  createServer(serverDemux, 1, "scope-1");
  createServer(serverDemux, 2, "scope-2");

  // 关闭 scope 1：两端都撤掉它的路由（生产由 detach 收口）。
  scope1Client.dispose();
  clientDemux.disposeScope(1);
  serverDemux.disposeScope(1);

  // 对端再发 scope 1 的帧：服务端没有该 scope，必须落进 droppedScopes，而不是误投 base。
  const scopedFrame = encodeScopedFrame(1, new Uint8Array([0x01]));
  transport.client.send(VSBuffer.wrap(scopedFrame));
  assert.deepEqual(serverDemux.droppedScopes, [1], "未知 scope 的帧必须丢弃");

  // base 与 scope 2 不受影响。
  assert.equal(await baseClient.getChannel<IChannel>("echo").call<string>("who"), "base");
  assert.equal(await scope2Client.getChannel<IChannel>("echo").call<string>("who"), "scope-2");

  // 关闭后 scope 1 的调用以 disposed 失败收口，而不是永久挂住。
  await assert.rejects(scope1Client.getChannel<IChannel>("echo").call<string>("who"), /disposed/u);
});
