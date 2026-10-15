/**
 * 编码能力缓存持久化夹具（spec: docs/specs/lan-remote-desktop-native-media.md
 * 「编码能力缓存」2026-10-13）：写→读回环、损坏文件降级、白名单收敛。
 * 纯 fs + 纯函数，跨平台可跑。
 */
import assert from "node:assert/strict";
import { mkdtemp, writeFile, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

const {
  ENCODER_CAPABILITY_FILE,
  loadPersistedEncoderCapability,
  persistEncoderCapability,
  sanitizeStoredEncoderCapability,
} = await import("../../src/host/encoderCapabilityStore.js");

// ── 白名单收敛（纯函数）──
assert.equal(sanitizeStoredEncoderCapability({ capability: "available" }), "available");
assert.equal(sanitizeStoredEncoderCapability({ capability: "unavailable" }), "unavailable");
assert.equal(sanitizeStoredEncoderCapability({ capability: "unknown" }), "unknown");
assert.equal(sanitizeStoredEncoderCapability({ capability: "yes" }), null, "未知枚举拒绝");
assert.equal(sanitizeStoredEncoderCapability({}), null, "缺键拒绝");
assert.equal(sanitizeStoredEncoderCapability("available"), null, "非对象拒绝");
assert.equal(sanitizeStoredEncoderCapability(null), null);

// ── 写读回环 ──
const dir = await mkdtemp(join(tmpdir(), "mode-rd-cap-"));
const path = join(dir, ENCODER_CAPABILITY_FILE);
assert.equal(await loadPersistedEncoderCapability(path), null, "缺失文件 = 无缓存");
await persistEncoderCapability(path, "available");
assert.equal(await loadPersistedEncoderCapability(path), "available", "回环读回");
// 落盘形状含 observedAt（诊断用）且 capability 原值可读。
const raw = JSON.parse(await readFile(path, "utf8")) as { capability?: unknown; observedAt?: unknown };
assert.equal(raw.capability, "available");
assert.equal(typeof raw.observedAt, "number");

// ── 损坏文件降级 + 覆盖写 ──
await writeFile(path, "{oops", "utf8");
assert.equal(await loadPersistedEncoderCapability(path), null, "损坏文件 = 无缓存");
await persistEncoderCapability(path, "unavailable");
assert.equal(await loadPersistedEncoderCapability(path), "unavailable", "真实会话观测覆盖旧值");

console.log("REMOTE_DESKTOP_ENCODER_CAPABILITY_OK");
