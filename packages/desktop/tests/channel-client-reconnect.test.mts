import assert from "node:assert/strict";
import test from "node:test";

import { connectViaProtocolWithClient } from "@mode/client";
import {
  ChannelServer,
  Emitter,
  VSBuffer,
  type IMessagePassingProtocol,
  type IServerChannel,
} from "@mode/rpc";

/** 两条内存协议互连；重连时复用同一对 protocol，只换新的 ChannelServer。 */
class MemoryProtocol implements IMessagePassingProtocol {
  private readonly emitter = new Emitter<VSBuffer>();
  readonly onMessage = this.emitter.event;
  peer: MemoryProtocol | null = null;

  send(buffer: VSBuffer): void {
    this.peer?.emitter.fire(buffer);
  }

  deliver(buffer: VSBuffer): void {
    this.emitter.fire(buffer);
  }
}

function createPair(): { client: MemoryProtocol; server: MemoryProtocol } {
  const client = new MemoryProtocol();
  const server = new MemoryProtocol();
  client.peer = server;
  server.peer = client;
  return { client, server };
}

class TestChannel implements IServerChannel<string> {
  private readonly events = new Emitter<number>();
  async call(_ctx: string, command: string): Promise<unknown> {
    if (command === "slow") return await new Promise(() => undefined);
    return { ok: true };
  }
  listen(_ctx: string): ReturnType<Emitter<number>["event"]> {
    return this.events.event;
  }
  fire(value: number): void {
    this.events.fire(value);
  }
}

async function waitFor(predicate: () => boolean, timeoutMs = 2_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error("等待 ChannelClient 重连超时");
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

test("ChannelClient 传输重建后拒绝在途请求并重放事件订阅", async () => {
  const protocols = createPair();
  const { accessor, client } = connectViaProtocolWithClient(protocols.client);
  const channel = new TestChannel();
  let server = new ChannelServer(protocols.server, "ctx");
  server.registerChannel("test", channel);
  const testChannel = accessor.systemService; // 只用于确保 accessor 已构建
  assert.ok(testChannel);

  const proxy = client.getChannel<{
    call<T>(command: string): Promise<T>;
    listen<T>(event: string): ReturnType<Emitter<T>["event"]>;
  }>("test");
  const events: number[] = [];
  const subscription = proxy.listen<number>("tick")((value) => events.push(value));
  channel.fire(1);
  await waitFor(() => events.includes(1));

  server.dispose();
  const slow = proxy.call("slow");
  client.notifyTransportClosed(new Error("transport lost"));
  await assert.rejects(slow, /transport lost/);

  // 新的 ChannelServer 会重新发 Initialize；客户端应重放 EventListen。
  server = new ChannelServer(protocols.server, "ctx");
  server.registerChannel("test", channel);
  // EventListen 由客户端在 Initialize 后重放，ChannelServer 的注册 flush 是宏任务。
  await new Promise((resolve) => setTimeout(resolve, 20));
  channel.fire(2);
  await waitFor(() => events.includes(2));
  subscription.dispose();
  server.dispose();
  client.dispose();
});
