/**
 * host 媒体中继回归：本地管道长度前缀分帧、kind 路由、media WS 上行（ready/ping/输入）、
 * 背压丢弃与超长帧防御。运行：
 * node --test packages/desktop/tests/remote-desktop-media-relay.test.mts
 * 扩展名 .mts：语言政策新测试禁 .mjs（docs/specs/language-policy.md）。
 * 直接 import src 用 .ts 扩展名（与 remote-workspace-session-services.test.mts 同口径）：
 * node --test 原生 type stripping 不做 .js → .ts 重写，简报里的 .js 写法在该入口下无法解析。
 * 每个用例用 try/finally 收口：断言失败时也必须关掉回环连接，否则进程句柄挂住不退。
 */
import assert from "node:assert/strict";
import test from "node:test";
import { connect, type Socket } from "node:net";
import { once } from "node:events";
import { createRemoteDesktopMediaRelay, type WsLike } from "../src/host/remoteDesktopMediaRelay.ts";

/** 管道帧：[u32 len 大端][u8 kind][payload]，len = 1 + payload.length（契约 B）。 */
function pipeFrame(kind: number, payload: Uint8Array): Buffer {
  const body = Buffer.concat([Buffer.from([kind]), Buffer.from(payload)]);
  const header = Buffer.alloc(4);
  header.writeUInt32BE(body.length, 0);
  return Buffer.concat([header, body]);
}

const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

const noopLogger = { info(): void {}, warn(): void {} };

/** 只收集帧的 fake media WS（WsLike 最小鸭子类型；bufferedAmount 由 buffered.amount 可变模拟）。 */
function makeFakeWs(): {
  ws: WsLike;
  texts: string[];
  binaries: Buffer[];
  buffered: { amount: number };
} {
  const texts: string[] = [];
  const binaries: Buffer[] = [];
  const buffered = { amount: 0 };
  const ws: WsLike = {
    send: (data) => {
      if (typeof data === "string") {
        texts.push(data);
      } else {
        binaries.push(Buffer.from(data));
      }
    },
    get bufferedAmount(): number {
      return buffered.amount;
    },
  };
  return { ws, texts, binaries, buffered };
}

/** 无论断言成败都关中继与 helper 连接，保证 node --test 进程能退出。 */
function cleanup(relay: { close(): void }, helper?: Socket): void {
  relay.close();
  helper?.destroy();
}

test("粘包/半包正确分帧；kind0 透传出口、kind1 触发 onStatus、kind2 下发开流", async () => {
  const seen: unknown[] = [];
  const relay = createRemoteDesktopMediaRelay({
    logger: noopLogger,
    onFrame: (p) => seen.push(["frame", Buffer.from(p).toString("hex")]),
    onStatus: (frozen, reason) => seen.push(["status", frozen, reason]),
    onReady: () => seen.push(["ready"]),
    onInput: (e) => seen.push(["input", e]),
  });
  const helper = connect({ host: "127.0.0.1", port: await relay.listen() });
  try {
    await once(helper, "connect");
    // payload 契约 B 形状：seq=7(00000007)、flags=1、密文两字节 → 7 字节原样透传
    const first = pipeFrame(0, Buffer.from([0, 0, 0, 7, 1, 9, 9]));
    const second = pipeFrame(1, Buffer.from(JSON.stringify({ frozen: true, reason: "locked" })));
    // 半包：先写一半第一个帧
    helper.write(first.subarray(0, 3));
    await sleep(10);
    helper.write(Buffer.concat([first.subarray(3), second]));
    await sleep(30);
    assert.deepEqual(seen, [
      ["frame", "00000007010909"],
      ["status", true, "locked"],
    ]);
  } finally {
    cleanup(relay, helper);
  }
});

test("WS 上行 ready 只下发一次 kind2；ping 立即 pong；输入透传", async () => {
  const kinds: number[] = [];
  const inputs: unknown[] = [];
  const relay = createRemoteDesktopMediaRelay({
    logger: noopLogger,
    onFrame: () => {},
    onStatus: () => {},
    onReady: () => {},
    onInput: (e) => inputs.push(e),
  });
  const helper = connect({ host: "127.0.0.1", port: await relay.listen() });
  try {
    await once(helper, "connect");
    helper.on("data", (d) => {
      kinds.push(d[4]!);
    });
    const { ws: fakeWs, texts } = makeFakeWs();
    relay.attachWebSocket(fakeWs);
    relay.handleWsMessage(JSON.stringify({ t: "ready" }), false);
    // 重复 ready 必须幂等：一条会话只发一次开流指令
    relay.handleWsMessage(JSON.stringify({ t: "ready" }), false);
    relay.handleWsMessage(JSON.stringify({ t: "ping", ts: 123 }), false);
    relay.handleWsMessage(JSON.stringify({ t: "pm", x: 10, y: 20 }), false);
    await sleep(30);
    assert.deepEqual(kinds, [2], "ready 只应下发一次 kind2");
    assert.deepEqual(texts, ['{"t":"pong","ts":123}'], "ping 立即回 pong 且原样回传 ts");
    assert.deepEqual(inputs, [{ t: "pm", x: 10, y: 20 }], "非 ready/ping 的上行对象透传 onInput");
  } finally {
    cleanup(relay, helper);
  }
});

test("kind0 缺省出口：无 WS 丢弃；bufferedAmount>4MB 背压丢弃；低于阈值即发送", async () => {
  const relay = createRemoteDesktopMediaRelay({
    logger: noopLogger,
    // 不注入 onFrame → 走 relay 缺省出口（契约 C：断开即丢帧、>4MB 丢 kind0）
    onStatus: () => {},
    onReady: () => {},
    onInput: () => {},
  });
  const helper = connect({ host: "127.0.0.1", port: await relay.listen() });
  try {
    await once(helper, "connect");
    const payloadA = Buffer.from([0, 0, 0, 1, 0, 0xaa]);
    const payloadB = Buffer.from([0, 0, 0, 1, 0, 0xbb]);

    // 未挂 WS：kind0 直接丢弃，不抛错（无从断言，只验证路径安全）
    helper.write(pipeFrame(0, payloadA));
    await sleep(20);

    const { ws: fakeWs, binaries, buffered } = makeFakeWs();
    buffered.amount = 5 * 1024 * 1024;
    relay.attachWebSocket(fakeWs);
    helper.write(pipeFrame(0, payloadA));
    await sleep(20);
    assert.equal(binaries.length, 0, "过 4MB 背压阈值应丢弃 kind0");

    buffered.amount = 0;
    helper.write(pipeFrame(0, payloadB));
    await sleep(20);
    assert.equal(binaries.length, 1, "低于阈值应恢复发送");
    assert.deepEqual(binaries[0], payloadB, "kind0 payload 原样转发（不解密不改写）");
  } finally {
    cleanup(relay, helper);
  }
});

test("超长帧防御：len > 16MB 关闭 helper 连接", async () => {
  const relay = createRemoteDesktopMediaRelay({
    logger: noopLogger,
    onStatus: () => {},
    onReady: () => {},
    onInput: () => {},
  });
  const helper = connect({ host: "127.0.0.1", port: await relay.listen() });
  try {
    await once(helper, "connect");
    const closed = once(helper, "close");
    const header = Buffer.alloc(4);
    header.writeUInt32BE(16 * 1024 * 1024 + 1, 0);
    helper.write(header);
    await Promise.race([closed, sleep(1000)]);
    assert.equal(helper.destroyed, true, "超长帧应导致连接被关闭");
  } finally {
    cleanup(relay, helper);
  }
});

test("ready 先于 helper 接入：开流指令挂起，helper 连通后补发且仍只发一次", async () => {
  const relay = createRemoteDesktopMediaRelay({
    logger: noopLogger,
    onFrame: () => {},
    onStatus: () => {},
    onReady: () => {},
    onInput: () => {},
  });
  const { ws: fakeWs } = makeFakeWs();
  const port = await relay.listen();
  let helper: Socket | undefined;
  try {
    // ready 先到（主控解码就绪可能早于 helper 管道接通，两者并行）
    relay.attachWebSocket(fakeWs);
    relay.handleWsMessage(JSON.stringify({ t: "ready" }), false);

    const kinds: number[] = [];
    helper = connect({ host: "127.0.0.1", port });
    await once(helper, "connect");
    helper.on("data", (d) => {
      kinds.push(d[4]!);
    });
    await sleep(30);
    assert.deepEqual(kinds, [2], "helper 接通后应补发开流指令");

    // 幂等不因补发而破坏：再收 ready 也不重发
    relay.handleWsMessage(JSON.stringify({ t: "ready" }), false);
    await sleep(30);
    assert.deepEqual(kinds, [2], "补发后重复 ready 仍不重发");
  } finally {
    cleanup(relay, helper);
  }
});

test("方向越权/未知 kind 不外发不回调；close 幂等", async () => {
  const seen: unknown[] = [];
  const relay = createRemoteDesktopMediaRelay({
    logger: noopLogger,
    onFrame: (p) => seen.push(["frame", Buffer.from(p)]),
    onStatus: (frozen) => seen.push(["status", frozen]),
    onReady: () => seen.push(["ready"]),
    onInput: (e) => seen.push(["input", e]),
  });
  try {
    // kind2 只允许 host→helper；helper 侧收到只 warn 不转发
    relay.handlePipeData(pipeFrame(2, Buffer.alloc(0)));
    // 未知 kind：丢弃，不触发任何出口回调
    relay.handlePipeData(pipeFrame(9, Buffer.from([1, 2, 3])));
    // kind1 非法 JSON：丢弃，不触发 onStatus
    relay.handlePipeData(pipeFrame(1, Buffer.from("not-json")));
    assert.deepEqual(seen, []);

    await relay.listen();
    relay.close();
    relay.close(); // 幂等：重复关闭不抛
  } finally {
    cleanup(relay);
  }
});
