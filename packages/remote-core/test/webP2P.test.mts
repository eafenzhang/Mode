/**
 * 浏览器 P2P 会话（WebP2PSession）：
 * 1. offer 经 Control 上行，answer 下行后 DataChannel 挂载成功；
 * 2. 没有 answer 时按超时返回 undefined，让调用方回落 Relay；
 * 3. ICE 服务器解析只接受带 urls 的条目。
 */
import assert from "node:assert/strict";
import test from "node:test";

import type {
  IceServerPayload,
  P2PAdapter,
  P2PChannel,
  P2PInitiator,
} from "../src/index.js";
import { readIceServers, WebP2PSession } from "../src/web/p2pSession.js";

function createChannel(): P2PChannel & { sent: string[] } {
  const sent: string[] = [];
  return {
    sent,
    send: (text) => {
      sent.push(text);
    },
    onData: () => undefined,
    onClose: () => undefined,
    close: () => undefined,
  };
}

class FakeInitiator implements P2PInitiator {
  readonly remoteCandidates: Array<{ candidate: string; sdpMid?: string }> = [];
  readonly opened = createChannel();
  private channelCallbacks: Array<(channel: P2PChannel) => void> = [];
  private localCallbacks: Array<(candidate: string, sdpMid?: string) => void> = [];
  private answers: string[] = [];

  async createOffer(): Promise<string> {
    // 模拟 ICE 候选：offer 生成后立即上报一条本地候选。
    for (const callback of this.localCallbacks) callback("candidate:local-1", "0");
    return "offer-sdp";
  }

  async acceptAnswer(sdp: string): Promise<void> {
    this.answers.push(sdp);
    for (const callback of this.channelCallbacks) callback(this.opened);
  }

  onLocalCandidate(callback: (candidate: string, sdpMid?: string) => void): void {
    this.localCallbacks.push(callback);
  }

  addRemoteCandidate(candidate: string, sdpMid?: string): void {
    this.remoteCandidates.push({ candidate, ...(sdpMid !== undefined ? { sdpMid } : {}) });
  }

  onChannel(callback: (channel: P2PChannel) => void): void {
    this.channelCallbacks.push(callback);
  }

  close(): void {
    // 测试里没有需要释放的原生资源。
  }

  get negotiatedAnswer(): string | undefined {
    return this.answers[0];
  }
}

function answerFrame(connectionId: string, sdp = "answer-sdp"): Parameters<WebP2PSession["dispatch"]>[0] {
  return {
    v: 1,
    id: "frame-answer",
    timestamp: Date.now(),
    type: "p2p.answer",
    payload: { connectionId, sdp },
  } as Parameters<WebP2PSession["dispatch"]>[0];
}

test("answer 下行后 WebP2PSession 返回可写 socket", async () => {
  const initiator = new FakeInitiator();
  const adapter: P2PAdapter = {
    createInitiator: () => initiator,
    createResponder: () => {
      throw new Error("browser client 不创建 responder");
    },
  };
  const sent: Array<{ type: string; payload: Record<string, unknown> }> = [];
  const session = new WebP2PSession({
    adapter,
    timeoutMs: 1_000,
    send: (type, payload) => sent.push({ type, payload }),
    onError: () => undefined,
  });
  const opening = session.open("conn-1", []);
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.equal(sent.some((frame) => frame.type === "p2p.offer"), true);
  assert.equal(sent.some((frame) => frame.type === "p2p.candidate"), true);

  // 候选通常先于 answer 到达；通道建立后的候选不再影响协商。
  session.handleCandidate("conn-1", "candidate:remote-1", "0");
  assert.deepEqual(initiator.remoteCandidates, [
    { candidate: "candidate:remote-1", sdpMid: "0" },
  ]);

  session.dispatch(answerFrame("conn-1"));
  const socket = await opening;
  assert.ok(socket, "answer 后必须拿到 DataChannel socket");
  assert.equal(initiator.negotiatedAnswer, "answer-sdp");

  socket.send('{"v":1}');
  assert.deepEqual(initiator.opened.sent, ['{"v":1}']);
  session.close();
});

test("没有 answer 时超时返回 undefined，不阻塞回落 Relay", async () => {
  let error: Error | undefined;
  const adapter: P2PAdapter = {
    createInitiator: () => new FakeInitiator(),
    createResponder: () => {
      throw new Error("browser client 不创建 responder");
    },
  };
  const session = new WebP2PSession({
    adapter,
    timeoutMs: 20,
    send: () => undefined,
    onError: (cause) => {
      error = cause;
    },
  });
  const socket = await session.open("conn-2", []);
  assert.equal(socket, undefined);
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.ok(error, "协商失败必须上报错误供日志记录");
  session.close();
});

test("readIceServers 只接受带字符串 urls 的条目", () => {
  const servers: IceServerPayload[] = readIceServers([
    { urls: ["stun:stun.example.com:3478"] },
    { urls: ["turn:turn.example.com:3478"], username: "u", credential: "c" },
    { urls: [] },
    { username: "u" },
    "not-an-object",
  ]);
  assert.deepEqual(servers, [
    { urls: ["stun:stun.example.com:3478"] },
    { urls: ["turn:turn.example.com:3478"], username: "u", credential: "c" },
  ]);
});
