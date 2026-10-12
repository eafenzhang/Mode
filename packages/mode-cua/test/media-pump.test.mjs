// Task 7（lan-remote-desktop-native-media）：helper 媒体泵（契约 B 管道 + 契约 A napi 面）。
// 不依赖真 addon/.node 产物：注入 fake addon（模拟 napi 面形状）+ 本机 net 回环扮演
// 被控 host 中继口。帧按字节逐字段断言（长度前缀/kind/seq/flags/密文/tag），并解密
// 回原文验证 IV=salt∥seq（IV 不随帧传，契约 B 要求解密方重建）。
import test from "node:test";
import assert from "node:assert/strict";
import { createDecipheriv } from "node:crypto";
import { createServer } from "node:net";

import { createMediaPump } from "../helper/mediaPump.mjs";

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function waitFor(cond, { timeoutMs = 2_000, intervalMs = 10 } = {}) {
  const deadline = Date.now() + timeoutMs;
  while (!cond()) {
    if (Date.now() > deadline) throw new Error("waitFor: condition not reached in time");
    await sleep(intervalMs);
  }
}

// 长度前缀分帧收集器（扮演 host 中继口的读取侧）：[u32 len 大端][body]，body = kind + payload。
// tail 单独暴露：泵写歪 len 时残留字节会被测试抓住。
function readFrames() {
  const frames = [];
  const lens = [];
  let accum = Buffer.alloc(0);
  return {
    frames,
    lens,
    tail: () => accum,
    attach(socket) {
      socket.on("data", (chunk) => {
        accum = Buffer.concat([accum, chunk]);
        while (accum.length >= 4) {
          const len = accum.readUInt32BE(0);
          if (accum.length < 4 + len) break;
          lens.push(len);
          frames.push(accum.subarray(4, 4 + len));
          accum = accum.subarray(4 + len);
        }
      });
    },
  };
}

// 回环中继口：collector 收帧；hostSides 暴露服务端连接（测试借此下发 kind2 / 断管）。
async function startRelay() {
  const server = createServer();
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const collector = readFrames();
  const hostSides = [];
  server.on("connection", (socket) => {
    hostSides.push(socket);
    collector.attach(socket);
  });
  return {
    port: server.address().port,
    collector,
    hostSides,
    close: () => new Promise((resolve) => server.close(resolve)),
  };
}

// 等 host 侧连接建立并下发 kind2 开流指令（ready 门控解闸）——所有推流断言的公共前置。
async function openStream(relay, index = 0) {
  await waitFor(() => relay.hostSides.length > index);
  relay.hostSides[index].write(Buffer.from([0, 0, 0, 1, 2])); // kind2 = [u32BE len=1][0x02]
  await sleep(50);
}

// 契约 A 的 napi 面形状：onFrame/onStatus 单参回调；switch 收 rect 本体（见实现文件注释）。
function fakeAddon(listeners) {
  return {
    rd_media_start(opts) {
      listeners.frame = opts.onFrame;
      listeners.status = opts.onStatus;
      listeners.startOpts = opts;
      return undefined;
    },
    rd_media_switch(rect) {
      listeners.switched = rect;
    },
    rd_media_stop() {},
  };
}

function u32be(n) {
  const b = Buffer.alloc(4);
  b.writeUInt32BE(n >>> 0, 0);
  return b;
}

// 解密 kind0 body（[kind][u32 seq][u8 flags][ct‖tag]）：IV = salt ∥ seq 由解密方重建。
// tag 校验失败会在此抛错——IV 推导或密文布局任何一处写歪，测试即红。
function decryptKind0(body, keyHex, saltHex) {
  const seq = body.readUInt32BE(1);
  const iv = Buffer.concat([Buffer.from(saltHex, "hex"), u32be(seq)]);
  const decipher = createDecipheriv("aes-256-gcm", Buffer.from(keyHex, "hex"), iv);
  decipher.setAuthTag(body.subarray(body.length - 16));
  const plain = Buffer.concat([
    decipher.update(body.subarray(6, body.length - 16)),
    decipher.final(),
  ]);
  return { seq, flags: body[5], plain };
}

function startParams(port, { keyHex = "11".repeat(32), saltHex = "22".repeat(8) } = {}) {
  return {
    rect: { x: 0, y: 0, width: 1920, height: 1080 },
    maxWidth: 1920,
    fps: 60,
    bitrateBps: 8_000_000,
    endpoint: { host: "127.0.0.1", port },
    key: keyHex,
    salt: saltHex,
  };
}

// 收口纪律：测试失败也要关掉回环 server 与泵，否则悬挂连接会卡住 test runner 进程。
async function withPump(listeners, body) {
  const relay = await startRelay();
  const pump = createMediaPump(fakeAddon(listeners));
  try {
    await body(relay, pump);
  } finally {
    await pump.stop();
    await relay.close();
  }
}

test("帧经 AES-256-GCM 加密后按长度前缀写出；IV = salt∥seq", async () => {
  const listeners = {};
  const keyHex = "11".repeat(32);
  const saltHex = "22".repeat(8);
  await withPump(listeners, async (relay, pump) => {
    await pump.start(startParams(relay.port, { keyHex, saltHex }));
    await openStream(relay); // kind2 前不推流（契约 B ready 门控），先解闸再回帧

    listeners.frame({ seq: 0, key: true, data: Buffer.from("hello-es") });
    await waitFor(() => relay.collector.frames.length === 1);

    const body = relay.collector.frames[0];
    assert.equal(body[0], 0, "kind=0");
    assert.equal(body.readUInt32BE(1), 0, "seq=0 大端");
    assert.equal(body[5], 1, "关键帧 flags=bit0");
    // len 契约：1（kind）+ 5（帧头 = seq4+flags1）+ 密文（GCM 流式：与明文等长）+ 16（tag）。
    assert.equal(relay.collector.lens[0], 1 + 5 + 8 + 16);
    assert.equal(relay.collector.tail().length, 0, "帧后不得有残留字节");

    const { seq, flags, plain } = decryptKind0(body, keyHex, saltHex);
    assert.equal(seq, 0);
    assert.equal(flags, 1);
    assert.equal(plain.toString(), "hello-es");
  });
});

test("ready 门控：kind2 前不推流；kind1 状态不受门控；kind2 后按新 seq 续流", async () => {
  const listeners = {};
  const keyHex = "ab".repeat(32);
  const saltHex = "cd".repeat(8);
  await withPump(listeners, async (relay, pump) => {
    await pump.start(startParams(relay.port, { keyHex, saltHex }));
    await waitFor(() => relay.hostSides.length === 1);

    // kind2 未到：帧被门控丢弃；kind1 状态必须能出（host 靠它转 freeze 信令，推流前也要）
    listeners.frame({ seq: 0, key: true, data: Buffer.from("gated-out") });
    listeners.status({ frozen: true, reason: "access-lost" });
    await sleep(50);
    assert.equal(relay.collector.frames.filter((f) => f[0] === 0).length, 0, "kind2 前不得推流");
    assert.equal(relay.collector.frames.length, 1, "kind1 不受 ready 门控");
    assert.deepEqual(JSON.parse(relay.collector.frames[0].subarray(1).toString("utf8")), {
      frozen: true,
      reason: "access-lost",
    });

    // host 下发开流指令；故意分两片写——TCP 分片丢 kind2 会让门控永远关着（静默不推流），
    // 攒包重组必须容忍。
    const hostSide = relay.hostSides[0];
    hostSide.write(Buffer.from([0, 0, 0, 1]));
    await sleep(20);
    hostSide.write(Buffer.from([2]));
    await sleep(50);

    listeners.frame({ seq: 1, key: false, data: Buffer.from("resumed!") });
    await waitFor(() => relay.collector.frames.filter((f) => f[0] === 0).length === 1);
    const body = relay.collector.frames.find((f) => f[0] === 0);
    const { seq, flags, plain } = decryptKind0(body, keyHex, saltHex);
    // seq=1 的 IV 尾 4 字节非零：能解出即证明 IV = salt∥seq 的大端推导正确。
    assert.equal(seq, 1);
    assert.equal(flags, 0, "非关键帧 flags=0");
    assert.equal(plain.toString(), "resumed!");
  });
});

test("stop 幂等；重复 start 先停旧泵；旧会话迟到回调不跨会话", async () => {
  const relay = await startRelay();
  const calls = [];
  const listeners = [];
  const addon = {
    rd_media_start(opts) {
      calls.push("start");
      listeners.push(opts.onFrame);
      return undefined;
    },
    rd_media_switch() {
      calls.push("switch");
    },
    rd_media_stop() {
      calls.push("stop");
    },
  };
  const pump = createMediaPump(addon);
  try {
    // 未启动：stop 安全返回且不触 addon（幂等纪律）
    assert.deepEqual(await pump.stop(), { ok: true });
    assert.deepEqual(calls, []);

    const params = startParams(relay.port);
    await pump.start(params);
    await pump.start(params);
    assert.deepEqual(calls, ["start", "stop", "start"], "重复 start 必须先停旧泵");
    await waitFor(() => relay.hostSides.length === 2);
    await waitFor(() => relay.hostSides[0].destroyed);

    // 会话 2 开流后：旧会话（TSFN 队列可能延迟投递）的迟到帧不得写进新管道
    await openStream(relay, 1);
    listeners[0]({ seq: 9, key: true, data: Buffer.from("stale") });
    listeners[1]({ seq: 2, key: true, data: Buffer.from("fresh") });
    await waitFor(() => relay.collector.frames.filter((f) => f[0] === 0).length === 1);
    await sleep(30);
    const kind0 = relay.collector.frames.filter((f) => f[0] === 0);
    assert.equal(kind0.length, 1, "迟到帧必须被会话身份过滤");
    assert.equal(decryptKind0(kind0[0], params.key, params.salt).plain.toString(), "fresh");

    assert.deepEqual(await pump.stop(), { ok: true });
    assert.deepEqual(calls, ["start", "stop", "start", "stop"]);
    // 再次 stop：幂等、不重复触 addon；stop 后的迟到帧不得再写
    assert.deepEqual(await pump.stop(), { ok: true });
    assert.deepEqual(calls, ["start", "stop", "start", "stop"]);
    listeners[1]({ seq: 3, key: true, data: Buffer.from("late") });
    await sleep(30);
    assert.equal(relay.collector.frames.filter((f) => f[0] === 0).length, 1);
  } finally {
    await pump.stop();
    await relay.close();
  }
});

test("connect 失败：start reject 且不触 addon（错误如实交 broker）", async () => {
  // 已关闭端口 = 必然拒连（拿一个临时监听口再关掉）
  const probe = createServer();
  await new Promise((resolve) => probe.listen(0, "127.0.0.1", resolve));
  const port = probe.address().port;
  await new Promise((resolve) => probe.close(resolve));

  const calls = [];
  const addon = {
    rd_media_start() {
      calls.push("start");
    },
    rd_media_switch() {
      calls.push("switch");
    },
    rd_media_stop() {
      calls.push("stop");
    },
  };
  const pump = createMediaPump(addon);
  await assert.rejects(pump.start(startParams(port)), (error) => error.code === "ECONNREFUSED");
  assert.deepEqual(calls, [], "connect 失败不得起采集");
  assert.deepEqual(await pump.stop(), { ok: true }, "失败后收口仍幂等");
});

test("管道被 host 断开：帧/状态写入不抛穿（TSFN Fatal 下不可炸进程），stop 仍幂等", async () => {
  const listeners = {};
  await withPump(listeners, async (relay, pump) => {
    await pump.start(startParams(relay.port));
    await waitFor(() => relay.hostSides.length === 1);

    relay.hostSides[0].destroy(); // 模拟 host 收口/异常断管
    await sleep(50);
    assert.doesNotThrow(() => listeners.frame({ seq: 0, key: true, data: Buffer.from("x") }));
    assert.doesNotThrow(() => listeners.status({ frozen: true, reason: "access-lost" }));
    assert.deepEqual(await pump.stop(), { ok: true });
    assert.deepEqual(await pump.stop(), { ok: true });
  });
});
