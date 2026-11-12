import assert from "node:assert/strict";
import test from "node:test";

// 原生媒体帧解析与 IV 派生（spec: lan-remote-desktop-native-media 协议节）。
const {
  computeFrameAgeMs,
  parseFrameHeader,
  buildFrameIv,
  codecStringFromAnnexB,
  shouldDropDeltaFrame,
} = await import("../src/remote-desktop/nativeMediaSession.js");

function frame(seq: number, key: boolean, body: Uint8Array, capturedAtMs = 0): Uint8Array {
  const out = new Uint8Array(9 + body.length);
  const view = new DataView(out.buffer);
  view.setUint32(0, seq, false);
  out[4] = key ? 1 : 0;
  view.setUint32(5, capturedAtMs >>> 0, false);
  out.set(body, 9);
  return out;
}

test("帧头解析：seq/flags/采集戳/body 切分正确，残包返回 null", () => {
  const f = frame(7, true, new Uint8Array([9, 9, 9]), 1234567);
  assert.deepEqual(parseFrameHeader(f), {
    seq: 7,
    key: true,
    capturedAtMs: 1234567,
    body: new Uint8Array([9, 9, 9]),
  });
  assert.equal(parseFrameHeader(f.subarray(0, 8)), null, "不足 9 字节头");
  assert.equal(parseFrameHeader(new Uint8Array(0)), null);
});

test("IV = salt(8B) ∥ seq(4B 大端)", () => {
  const salt = new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8]);
  const iv = buildFrameIv(salt, 0x01020304);
  assert.equal(iv.length, 12);
  assert.deepEqual([...iv], [1, 2, 3, 4, 5, 6, 7, 8, 1, 2, 3, 4]);
});

test("codecStringFromAnnexB：从 SPS 推导 avc1.PPCCLL", () => {
  // SPS: 00 00 00 01 67 42 00 1f ... → profile 0x42(66) level 0x1f(31)
  const sps = new Uint8Array([0, 0, 0, 1, 0x67, 0x42, 0x00, 0x1f, 0xaa, 0xbb]);
  assert.equal(codecStringFromAnnexB(sps), "avc1.42001f");
  assert.equal(
    codecStringFromAnnexB(new Uint8Array([0, 0, 0, 1, 0x65, 0, 0])),
    null,
    "无 SPS 返回 null",
  );
});

test("codecStringFromAnnexB：HEVC 码流派生 hev1.*（码流自描述，HEVC 选型）", () => {
  // HEVC NAL 头 = 2 字节（42 01）。SPS 载荷：vps_id/max_sub_layers/nesting=0x01；
  // ptl 首字节 = space0+tier0+idc1 → 0x01；compat u32 = 0x60000000（Main 经典值）
  // → EP 前为 60 00 00 03 00…；约束 4 位 = progressive|frame_only → 0x90；level 120。
  const sps = new Uint8Array([
    0, 0, 0, 1, 0x42, 0x01, 0x01, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0, 0, 0, 0, 0, 0x78,
    0xaa,
  ]);
  assert.equal(codecStringFromAnnexB(sps), "hev1.1.60000000.L120.90", "SPS 直接判族并解析");

  // VPS(40 01) 在前：向后扫到 SPS；tier=1(H)、约束 4 位全 0 → 省略约束段。
  const vpsSps = new Uint8Array([
    0,
    0,
    0,
    1,
    0x40,
    0x01,
    0x0c,
    0x01, // VPS（2 字节头 + 任意载荷，只判族）
    0,
    0,
    0,
    1,
    0x42,
    0x01,
    0x01,
    0x21,
    0x00,
    0x00,
    0x00,
    0x06,
    0x00,
    0,
    0,
    0,
    0,
    0,
    0x5d,
  ]);
  assert.equal(codecStringFromAnnexB(vpsSps), "hev1.1.6.H93", "VPS 先行 + tier H + 无约束段");

  // 截断 SPS → null（走「keyframe without SPS」分支，不抛）。
  assert.equal(codecStringFromAnnexB(new Uint8Array([0, 0, 0, 1, 0x42, 0x01])), null);
  // H.264 码流不得误判 HEVC（族判定只认 byte0 type 32/33）。
  assert.equal(
    codecStringFromAnnexB(new Uint8Array([0, 0, 0, 1, 0x67, 0x42, 0x00, 0x1f, 0xaa])),
    "avc1.42001f",
    "族判定不串扰",
  );
});

test("解码队列门控：队列 >2 丢 delta、关键帧永远放行", () => {
  assert.equal(shouldDropDeltaFrame(false, 0), false, "空队列不丢");
  assert.equal(shouldDropDeltaFrame(false, 2), false, "水位 2 内不丢");
  assert.equal(shouldDropDeltaFrame(false, 3), true, "队列 >2 丢 delta");
  assert.equal(shouldDropDeltaFrame(true, 10), false, "关键帧不受队列门控（重同步点）");
});

test("帧龄：u32 截断采集戳按 32 位回绕还原真实帧龄", () => {
  // 真机缺陷回归：Date.now() ~1.79e12，采集戳被截断成 ~3.9e9；
  // 直接相减得到 ~1.79e12，回绕后必须还原为真实毫秒差。
  const now = 1_791_001_362_450;
  const capturedTruncated = now >>> 0;
  assert.equal(computeFrameAgeMs(now + 120, capturedTruncated, 0), 120);
});

test("帧龄：时钟偏移参与回绕计算", () => {
  const now = 1_791_001_362_450;
  const capturedTruncated = (now - 80) >>> 0;
  // host 时钟比 client 快 500ms：帧龄 = 80 + 500。
  assert.equal(computeFrameAgeMs(now, capturedTruncated, 500), 580);
});

test("帧龄：偏移失准回绕出巨大值时返回 null，不污染统计", () => {
  const now = 1_791_001_362_450;
  assert.equal(computeFrameAgeMs(now, 12345, 0), null);
  // 负帧龄回绕（偏移误差大于真实帧龄）同样拒绝。
  const capturedTruncated = (now + 50) >>> 0;
  assert.equal(computeFrameAgeMs(now, capturedTruncated, 0), null);
});
