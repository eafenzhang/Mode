import assert from "node:assert/strict";
import test from "node:test";

// 编码偏好降级（spec: lan-remote-desktop-native-media.md「媒体面可观测性与首帧约束」）：
// 预检通过 ≠ 真能解——HEVC 解码失败时降为 H.264 并放行一次整会话重建，而不是把会话判死。
const { applyCodecProbe, stepCodecDowngrade, INITIAL_CODEC_FALLBACK_STATE } =
  await import("../src/remote-desktop/codecFallback.js");

test("预检落地：只落一次，支持与否分别得 hevc/h264", () => {
  assert.equal(INITIAL_CODEC_FALLBACK_STATE.codec, null);
  assert.equal(applyCodecProbe(INITIAL_CODEC_FALLBACK_STATE, true).codec, "hevc");
  assert.equal(applyCodecProbe(INITIAL_CODEC_FALLBACK_STATE, false).codec, "h264");
  // 已落地的偏好不被后来的预检覆盖（预检只做一次）。
  assert.equal(applyCodecProbe({ codec: "hevc" }, false).codec, "hevc");
});

test("解码失败降级：HEVC → 降 H.264 且只降一次；其余情形不动", () => {
  const hevc = applyCodecProbe(INITIAL_CODEC_FALLBACK_STATE, true);
  const first = stepCodecDowngrade(hevc);
  assert.equal(first.downgrade, true, "HEVC 解码失败要降级重建");
  assert.equal(first.state.codec, "h264");

  // 降级后的 h264 会话再失败：不再降级（交既有失败归因），也不回环到 HEVC。
  const second = stepCodecDowngrade(first.state);
  assert.equal(second.downgrade, false, "h264 失败不再重建");
  assert.equal(second.state.codec, "h264");

  // 原生 H.264 与未预检：都不是降级场景。
  assert.equal(stepCodecDowngrade({ codec: "h264" }).downgrade, false);
  assert.equal(stepCodecDowngrade(INITIAL_CODEC_FALLBACK_STATE).downgrade, false);
});

test("降级后预检不再把偏好拉回 HEVC（单向）", () => {
  const downgraded = stepCodecDowngrade(applyCodecProbe(INITIAL_CODEC_FALLBACK_STATE, true)).state;
  assert.equal(applyCodecProbe(downgraded, true).codec, "h264");
});
