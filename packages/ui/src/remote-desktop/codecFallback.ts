/**
 * 编码 codec 偏好与其降级状态（spec: docs/specs/lan-remote-desktop-native-media.md「媒体面
 * 可观测性与首帧约束」；HEVC 选型见事件顺序 ①）。
 *
 * 主控的 codec 偏好来自本机解码能力预检（`probeHevcDecodeSupport`）：预检说支持就一路 HEVC。
 * 但预检通过 ≠ 真能解——profile / 位深 / 驱动差异会让实际解码（configure 或 decode）失败，
 * 而那时主控原本只会按「媒体失败」把会话判死，用户看到的是「连接失败」而不是「换编码就好」。
 * 本模块只做一件事：解码失败时把偏好降为 H.264，并让会话胶水放行一次整会话重建。
 *
 * 单向：降级后不再回到 HEVC（同一挂载周期内不做回环重试）；只降一次（再次失败走既有归因）。
 */
import type { RemoteDesktopCodec } from "@mode/shared/remote-desktop";

export interface CodecFallbackState {
  /** 当前偏好：null = 尚未预检。 */
  codec: RemoteDesktopCodec | null;
}

export const INITIAL_CODEC_FALLBACK_STATE: CodecFallbackState = { codec: null };

/** 预检结果落地（只落一次；已降级为 h264 时不会被后来的预检覆盖回 HEVC）。 */
export function applyCodecProbe(
  state: CodecFallbackState,
  hevcSupported: boolean,
): CodecFallbackState {
  if (state.codec !== null) {
    return state;
  }
  return { codec: hevcSupported ? "hevc" : "h264" };
}

/**
 * 解码失败 → 是否降级重建：只有「当前是 HEVC」才降（未预检 / 已是 H.264 / 已降过都不动）。
 * 返回降级后的状态与是否该重建会话（调用方据此抑制本次失败归因并重连）。
 */
export function stepCodecDowngrade(state: CodecFallbackState): {
  state: CodecFallbackState;
  downgrade: boolean;
} {
  if (state.codec !== "hevc") {
    return { state, downgrade: false };
  }
  return { state: { codec: "h264" }, downgrade: true };
}
