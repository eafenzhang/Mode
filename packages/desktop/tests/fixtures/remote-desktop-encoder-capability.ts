/**
 * 编码能力缓存夹具（spec: docs/specs/lan-remote-desktop-native-media.md
 * 「编码能力缓存」2026-10-13 / 「软件编码回退」2026-10-15）：写→读回环、损坏文件降级、
 * 白名单收敛，以及 rd_media_start 结果 → 能力取值的映射（software 态）。
 * 纯 fs + 纯函数，跨平台可跑。
 */
import assert from "node:assert/strict";
import { mkdtemp, writeFile, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

const {
  ENCODER_CAPABILITY_FILE,
  encoderCapabilityFromStartResult,
  loadPersistedEncoderCapability,
  persistEncoderCapability,
  sanitizeStoredEncoderCapability,
} = await import("../../src/host/encoderCapabilityStore.js");

// ── 白名单收敛（纯函数）──
assert.equal(sanitizeStoredEncoderCapability({ capability: "available" }), "available");
assert.equal(sanitizeStoredEncoderCapability({ capability: "software" }), "software");
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

// ── 损坏文件降级 + 覆盖写（含 software 态）──
await writeFile(path, "{oops", "utf8");
assert.equal(await loadPersistedEncoderCapability(path), null, "损坏文件 = 无缓存");
await persistEncoderCapability(path, "software");
assert.equal(await loadPersistedEncoderCapability(path), "software", "软件回退观测落盘可回读");
await persistEncoderCapability(path, "unavailable");
assert.equal(await loadPersistedEncoderCapability(path), "unavailable", "真实会话观测覆盖旧值");

// ── rd_media_start 结果 → 能力取值（2026-10-15 软件回退）──
// helper 回报 software = 本机无硬编、已回退软件编码：能出画面，但要提示代价（不是失败）。
assert.equal(
  encoderCapabilityFromStartResult({ encoder: "software" }),
  "software",
  "软件回退必须记 software（不得报成 available，否则设置页不给代价提示）",
);
assert.equal(encoderCapabilityFromStartResult({ encoder: "hardware" }), "available");
// 旧 helper 不带字段 / 形状异常：会话已建流成功，只能落「可用」，既不猜软件也不误报失败。
assert.equal(encoderCapabilityFromStartResult({}), "available", "缺字段 = available（不猜）");
assert.equal(encoderCapabilityFromStartResult(undefined), "available");
assert.equal(encoderCapabilityFromStartResult(null), "available");
assert.equal(encoderCapabilityFromStartResult("software"), "available", "非对象形状不采信");

console.log("REMOTE_DESKTOP_ENCODER_CAPABILITY_OK");
