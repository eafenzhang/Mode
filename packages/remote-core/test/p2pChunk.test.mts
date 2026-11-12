/**
 * P2P DataChannel 分片：单条消息受 SCTP/浏览器 256KB 左右限制，业务帧可能达 4MB。
 * 验证 p2pChannelAsSocket 对超限消息分片、对端按序拼回，且小消息保持原样。
 */
import assert from "node:assert/strict";
import test from "node:test";

import { p2pChannelAsSocket, type P2PChannel } from "../src/index.js";

class MemoryChannel implements P2PChannel {
  peer!: MemoryChannel;
  readonly sent: string[] = [];
  private readonly dataListeners: Array<(text: string) => void> = [];
  private readonly closeListeners: Array<(reason: string) => void> = [];

  send(text: string): void {
    this.sent.push(text);
    for (const listener of this.peer.dataListeners) listener(text);
  }

  onData(callback: (text: string) => void): void {
    this.dataListeners.push(callback);
  }

  onClose(callback: (reason: string) => void): void {
    this.closeListeners.push(callback);
  }

  close(reason?: string): void {
    for (const listener of this.closeListeners) listener(reason ?? "closed");
  }

  emit(text: string): void {
    for (const listener of this.dataListeners) listener(text);
  }
}

function pair(): { a: MemoryChannel; b: MemoryChannel } {
  const a = new MemoryChannel();
  const b = new MemoryChannel();
  a.peer = b;
  b.peer = a;
  return { a, b };
}

test("超过 64KB 的业务帧分片发送并在对端还原", () => {
  const { a, b } = pair();
  const sender = p2pChannelAsSocket(a);
  const receiver = p2pChannelAsSocket(b);
  const received: string[] = [];
  receiver.onMessage((text) => received.push(text));

  const payload = JSON.stringify({
    v: 1,
    type: "relay",
    payload: { ciphertext: `${"x".repeat(200_000)}中文🙂` },
  });
  sender.send(payload);

  assert.equal(a.sent.length > 1, true, "超限消息必须拆成多条");
  assert.equal(a.sent[0]!.startsWith('{"__p2pChunk":1,'), true);
  assert.deepEqual(received, [payload]);
});

test("小消息保持单帧直发，不做分片包装", () => {
  const { a, b } = pair();
  const sender = p2pChannelAsSocket(a);
  const receiver = p2pChannelAsSocket(b);
  const received: string[] = [];
  receiver.onMessage((text) => received.push(text));

  sender.send('{"v":1,"type":"ping"}');
  assert.deepEqual(a.sent, ['{"v":1,"type":"ping"}']);
  assert.deepEqual(received, ['{"v":1,"type":"ping"}']);
});

test("分片缺失时不投递半条消息", () => {
  // 只观察发送侧，接收侧手工投递，避免真实通道自动送达完整分片。
  const a = new MemoryChannel();
  a.peer = new MemoryChannel();
  const b = new MemoryChannel();
  const sender = p2pChannelAsSocket(a);
  const receiver = p2pChannelAsSocket(b);
  const received: string[] = [];
  receiver.onMessage((text) => received.push(text));

  const payload = "y".repeat(200_000);
  sender.send(payload);
  const frames = [...a.sent];
  assert.equal(frames.length >= 3, true);
  // 模拟丢帧：只投递首片与末片。
  b.emit(frames[0]!);
  b.emit(frames[frames.length - 1]!);
  assert.deepEqual(received, []);
});
