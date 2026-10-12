import assert from "node:assert/strict";
import test from "node:test";

// 原生媒体帧解析与 IV 派生（spec: lan-remote-desktop-native-media 协议节）。
const { parseFrameHeader, buildFrameIv, codecStringFromAnnexB, shouldDropDeltaFrame } =
  await import("../src/remote-desktop/nativeMediaSession.js");

function frame(seq: number, key: boolean, body: Uint8Array): Uint8Array {
  const out = new Uint8Array(5 + body.length);
  new DataView(out.buffer).setUint32(0, seq, false);
  out[4] = key ? 1 : 0;
  out.set(body, 5);
  return out;
}

test("帧头解析：seq/flags/body 切分正确，残包返回 null", () => {
  const f = frame(7, true, new Uint8Array([9, 9, 9]));
  const parsed = parseFrameHeader(f);
  assert.deepEqual(parsed, { seq: 7, key: true, body: new Uint8Array([9, 9, 9]) });
  assert.equal(parseFrameHeader(f.subarray(0, 4)), null, "不足 5 字节头");
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

test("解码队列门控：队列 >2 丢 delta、关键帧永远放行", () => {
  assert.equal(shouldDropDeltaFrame(false, 0), false, "空队列不丢");
  assert.equal(shouldDropDeltaFrame(false, 2), false, "水位 2 内不丢");
  assert.equal(shouldDropDeltaFrame(false, 3), true, "队列 >2 丢 delta");
  assert.equal(shouldDropDeltaFrame(true, 10), false, "关键帧不受队列门控（重同步点）");
});
