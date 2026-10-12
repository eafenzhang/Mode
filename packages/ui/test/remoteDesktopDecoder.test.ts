import assert from "node:assert/strict";
import { createCipheriv } from "node:crypto";
import test from "node:test";

// 解码胶水（WebCodecs + WS + WebCrypto）行为（spec docs/specs/lan-remote-desktop-native-media.md
// 「主控 renderer」节与协议节帧布局）：stub 掉 WebSocket/VideoDecoder/EncodedVideoChunk/canvas，
// 在 node 里用 node:crypto 构造契约 B 密文帧驱动 createNativeMediaSession。
// 纯函数面（帧头/IV/SPS）另见 remoteDesktopNativeSession.test.ts。

const KEY_HEX = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
const SALT_HEX = "0102030405060708";
const WRONG_KEY_HEX = "ff".repeat(32);

// SPS（profile 0x42 / constraint 0x00 / level 0x1f）→ avc1.42001f；IDR NAL type 5、delta NAL type 1。
const SPS = [0, 0, 0, 1, 0x67, 0x42, 0x00, 0x1f, 0xaa, 0xbb];
// level 0x28(40) 变体：换分辨率类 SPS 变化 → avc1.420028，触发重新 configure。
const SPS_HIGH_LEVEL = [0, 0, 0, 1, 0x67, 0x42, 0x00, 0x28, 0xaa, 0xbb];
const KEY_NAL = [0, 0, 0, 1, 0x65, 0x11, 0x22];
const DELTA_NAL = [0, 0, 0, 1, 0x41, 0x33];

function u32be(n: number): Buffer {
  const b = Buffer.alloc(4);
  b.writeUInt32BE(n >>> 0, 0);
  return b;
}

/** 与 helper 生产侧同口径构造 [u32 seq 大端][u8 flags][ciphertext‖tag]（IV = salt ∥ seq 大端）。 */
function frameBytes(
  seq: number,
  key: boolean,
  plain: number[],
  keyHex: string = KEY_HEX,
): ArrayBuffer {
  const iv = Buffer.concat([Buffer.from(SALT_HEX, "hex"), u32be(seq)]);
  const cipher = createCipheriv("aes-256-gcm", Buffer.from(keyHex, "hex"), iv);
  const ct = Buffer.concat([cipher.update(Buffer.from(plain)), cipher.final()]);
  const out = Buffer.alloc(5 + ct.length + 16);
  out.writeUInt32BE(seq >>> 0, 0);
  out.writeUInt8(key ? 1 : 0, 4);
  ct.copy(out, 5);
  cipher.getAuthTag().copy(out, 5 + ct.length);
  return out.buffer.slice(out.byteOffset, out.byteOffset + out.byteLength) as ArrayBuffer;
}

class StubWebSocket {
  static readonly OPEN = 1;
  static instances: StubWebSocket[] = [];
  readyState = StubWebSocket.OPEN;
  binaryType = "blob";
  sent: string[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: string | ArrayBuffer }) => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: ((event: unknown) => void) | null = null;
  constructor(readonly url: string) {
    StubWebSocket.instances.push(this);
  }
  send(data: string) {
    this.sent.push(data);
  }
  close(_code?: number, _reason?: string) {
    this.readyState = 3;
  }
  /** 测试驱动：模拟 WS open。 */
  fireOpen() {
    this.onopen?.();
  }
  /** 测试驱动：模拟下行消息（二进制帧 / 文本 JSON）。 */
  deliver(data: string | ArrayBuffer) {
    this.onmessage?.({ data });
  }
}

interface DecodeRecord {
  type: string;
  timestamp: number;
  data: Uint8Array;
}

class StubVideoDecoder {
  static instances: StubVideoDecoder[] = [];
  static decoded: DecodeRecord[] = [];
  state = "unconfigured";
  configs: { codec: string; optimizeForLatency?: boolean }[] = [];
  private readonly init: { output: (frame: unknown) => void; error: (error: unknown) => void };
  constructor(init: { output: (frame: unknown) => void; error: (error: unknown) => void }) {
    this.init = init;
    StubVideoDecoder.instances.push(this);
  }
  configure(config: { codec: string; optimizeForLatency?: boolean }) {
    if (this.state === "closed") throw new Error("decoder closed");
    this.configs.push(config);
    this.state = "configured";
  }
  decode(chunk: { type: string; timestamp: number; data: Uint8Array }) {
    if (this.state !== "configured") throw new TypeError("decoder is not configured");
    StubVideoDecoder.decoded.push({
      type: chunk.type,
      timestamp: chunk.timestamp,
      data: chunk.data,
    });
  }
  close() {
    if (this.state === "closed") throw new Error("already closed");
    this.state = "closed";
  }
  /** 测试驱动：模拟一次解码输出（画布/回调路径）。 */
  emitFrame(width: number, height: number) {
    this.init.output({ displayWidth: width, displayHeight: height, close: () => {} });
  }
}

class StubEncodedVideoChunk {
  readonly type: string;
  readonly timestamp: number;
  readonly data: Uint8Array;
  constructor(init: { type: string; timestamp: number; data: Uint8Array }) {
    this.type = init.type;
    this.timestamp = init.timestamp;
    this.data = init.data;
  }
}

globalThis.WebSocket = StubWebSocket as unknown as typeof WebSocket;
globalThis.VideoDecoder = StubVideoDecoder as unknown as typeof VideoDecoder;
globalThis.EncodedVideoChunk = StubEncodedVideoChunk as unknown as typeof EncodedVideoChunk;

const { createNativeMediaSession } = await import("../src/remote-desktop/nativeMediaSession.js");
type NativeMediaSession = ReturnType<typeof createNativeMediaSession>;

function createCanvasStub() {
  const draws: unknown[] = [];
  const canvas = {
    width: 0,
    height: 0,
    getContext: () => ({ drawImage: (frame: unknown) => draws.push(frame) }),
  };
  return { canvas, draws };
}

function createHarness() {
  StubWebSocket.instances = [];
  StubVideoDecoder.instances = [];
  StubVideoDecoder.decoded = [];
  const { canvas, draws } = createCanvasStub();
  const configuredCalls: { width: number; height: number; codec: string }[] = [];
  const counts = { decoded: 0, transportClosed: 0 };
  const session: NativeMediaSession = createNativeMediaSession({
    url: "ws://127.0.0.1:9/ws/remote-desktop-media?mediaTicket=stub",
    keyHex: KEY_HEX,
    saltHex: SALT_HEX,
    canvas: canvas as unknown as HTMLCanvasElement,
    onConfigured: (info) => configuredCalls.push(info),
    onDecoded: () => {
      counts.decoded += 1;
    },
    onTransportClosed: () => {
      counts.transportClosed += 1;
    },
  });
  const ws = StubWebSocket.instances[0]!;
  return { session, ws, canvas, draws, configuredCalls, counts };
}

/** 帧处理 = 同步分派 → 异步解密 → 续体：等若干宏任务让 WebCrypto 的 promise 全部落定。 */
async function settle(): Promise<void> {
  for (let i = 0; i < 5; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

test("ws open 即发 ready 恰一次；后续 send 不重复发", () => {
  const h = createHarness();
  assert.deepEqual(h.ws.sent, [], "open 前不上行");
  h.ws.fireOpen();
  assert.deepEqual(h.ws.sent, ['{"t":"ready"}']);
  h.session.send({ t: "pm", x: 1 });
  assert.deepEqual(
    h.ws.sent,
    ['{"t":"ready"}', '{"t":"pm","x":1}'],
    "send 不再补发 ready（幂等保险不重复）",
  );
  h.session.ping(1234);
  assert.deepEqual(h.ws.sent.at(-1), '{"t":"ping","ts":1234}');
});

test("首关键帧建立 decoder 并以 SPS 推导的 codec 配置", async () => {
  const h = createHarness();
  h.ws.fireOpen();
  h.ws.deliver(frameBytes(1, true, [...SPS, ...KEY_NAL]));
  await settle();
  assert.equal(StubVideoDecoder.instances.length, 1, "关键帧应建立 decoder");
  assert.deepEqual(StubVideoDecoder.instances[0]!.configs, [
    { codec: "avc1.42001f", optimizeForLatency: true },
  ]);
  assert.deepEqual(
    StubVideoDecoder.decoded.map((d) => d.type),
    ["key"],
  );
});

test("关键帧 + delta：解码次数、chunk 类型与时间戳单调", async () => {
  const h = createHarness();
  h.ws.fireOpen();
  h.ws.deliver(frameBytes(1, true, [...SPS, ...KEY_NAL]));
  h.ws.deliver(frameBytes(2, false, DELTA_NAL));
  await settle();
  assert.deepEqual(
    StubVideoDecoder.decoded.map((d) => d.type),
    ["key", "delta"],
  );
  const [first, second] = StubVideoDecoder.decoded;
  assert.ok(second!.timestamp > first!.timestamp, "时间戳必须单调递增");
});

test("decoder 未配置前到达的 delta 帧被丢弃（不建 decoder、不抛）", async () => {
  const h = createHarness();
  h.ws.fireOpen();
  h.ws.deliver(frameBytes(7, false, DELTA_NAL));
  await settle();
  assert.equal(StubVideoDecoder.instances.length, 0);
  assert.equal(StubVideoDecoder.decoded.length, 0);
  assert.equal(h.counts.transportClosed, 0, "丢单帧不得收口");
});

test("解密失败帧单帧丢弃：篡改 tag / 错误密钥都不炸，后续合法帧照常解码", async () => {
  const h = createHarness();
  h.ws.fireOpen();
  const tampered = new Uint8Array(frameBytes(1, true, [...SPS, ...KEY_NAL]));
  tampered[tampered.length - 1]! ^= 0xff;
  h.ws.deliver(tampered.buffer as ArrayBuffer);
  h.ws.deliver(frameBytes(2, true, [...SPS, ...KEY_NAL], WRONG_KEY_HEX));
  await settle();
  assert.equal(StubVideoDecoder.instances.length, 0, "坏帧不建 decoder");
  assert.equal(StubVideoDecoder.decoded.length, 0);
  assert.equal(h.counts.transportClosed, 0, "单帧解密失败不得收口");

  h.ws.deliver(frameBytes(3, true, [...SPS, ...KEY_NAL]));
  await settle();
  assert.deepEqual(
    StubVideoDecoder.decoded.map((d) => d.type),
    ["key"],
    "合法帧仍能解码",
  );
});

test("stop() 收口：decoder 关闭、通知恰一次、不再解码也不复活", async () => {
  const h = createHarness();
  h.ws.fireOpen();
  h.ws.deliver(frameBytes(1, true, [...SPS, ...KEY_NAL]));
  await settle();
  assert.equal(StubVideoDecoder.decoded.length, 1);

  h.session.stop();
  assert.equal(StubVideoDecoder.instances[0]!.state, "closed");
  assert.equal(h.counts.transportClosed, 1);

  // 迟到消息（stop 后仍可能被投递）：既不解码，也不复活 decoder。
  h.ws.deliver(frameBytes(2, true, [...SPS, ...KEY_NAL]));
  await settle();
  assert.equal(StubVideoDecoder.decoded.length, 1);
  assert.equal(StubVideoDecoder.instances.length, 1);

  // 收口幂等：后续 close/error 回调不重复通知。
  h.ws.onclose?.();
  h.ws.onerror?.();
  assert.equal(h.counts.transportClosed, 1);
});

test("关键帧 SPS 变化才重配 decoder（同 SPS 不重建，旧 decoder 关闭）", async () => {
  const h = createHarness();
  h.ws.fireOpen();
  h.ws.deliver(frameBytes(1, true, [...SPS, ...KEY_NAL]));
  h.ws.deliver(frameBytes(2, true, [...SPS, ...KEY_NAL]));
  await settle();
  assert.equal(StubVideoDecoder.instances.length, 1, "同 codec 串不重建");
  assert.deepEqual(
    StubVideoDecoder.decoded.map((d) => d.type),
    ["key", "key"],
  );

  h.ws.deliver(frameBytes(3, true, [...SPS_HIGH_LEVEL, ...KEY_NAL]));
  await settle();
  assert.equal(StubVideoDecoder.instances.length, 2, "SPS 变化应重配");
  assert.deepEqual(StubVideoDecoder.instances[1]!.configs, [
    { codec: "avc1.420028", optimizeForLatency: true },
  ]);
  assert.equal(StubVideoDecoder.instances[0]!.state, "closed", "旧 decoder 已关闭");
  assert.deepEqual(
    StubVideoDecoder.decoded.map((d) => d.type),
    ["key", "key", "key"],
  );
});

test("解码输出即画：画布随帧尺寸更新，onConfigured 只在尺寸变化时回调", async () => {
  const h = createHarness();
  h.ws.fireOpen();
  h.ws.deliver(frameBytes(1, true, [...SPS, ...KEY_NAL]));
  await settle();
  const decoder = StubVideoDecoder.instances[0]!;

  decoder.emitFrame(1920, 1080);
  assert.deepEqual(
    { width: h.canvas.width, height: h.canvas.height },
    { width: 1920, height: 1080 },
  );
  assert.deepEqual(h.configuredCalls, [{ width: 1920, height: 1080, codec: "avc1.42001f" }]);
  assert.equal(h.draws.length, 1);
  assert.equal(h.counts.decoded, 1);

  decoder.emitFrame(1920, 1080);
  assert.equal(h.configuredCalls.length, 1, "同尺寸不重复回调");

  decoder.emitFrame(1280, 720);
  assert.deepEqual(h.configuredCalls.at(-1), { width: 1280, height: 720, codec: "avc1.42001f" });
  assert.equal(h.canvas.width, 1280);
});
