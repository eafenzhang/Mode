/**
 * 视频编解码协商偏好（spec 产品规则 8 与迁移兼容性：保持标准 SDP，优先 H.264、VP8 兜底）。
 *
 * `RTCPeerConnection.getCapabilities` / `setCodecPreferences` 不在当前 TS DOM lib 里，
 * 且部分 Electron/Chromium 版本缺失或对非法列表抛错：两者都做特性探测 + try/catch，
 * 失败即回退浏览器默认协商顺序（spike 默认 VP8 已验证可用）。
 */
import { logger } from "@/logger.js";

/** 只用到 mimeType 的最小编解码能力形状（`RTCRtpCodecCapability` 不在当前 TS DOM lib 里）。 */
type RemoteDesktopCodecCapability = { mimeType: string };

export function applyPreferredRemoteDesktopVideoCodecs(pc: RTCPeerConnection): void {
  try {
    const capabilitiesOf = (
      RTCPeerConnection as unknown as {
        getCapabilities?: (kind: string) =>
          | {
              codecs?: RemoteDesktopCodecCapability[];
            }
          | undefined;
      }
    ).getCapabilities;
    const codecs =
      typeof capabilitiesOf === "function"
        ? (capabilitiesOf.call(RTCPeerConnection, "video")?.codecs ?? [])
        : [];
    if (codecs.length === 0) {
      return;
    }
    const setCodecPreferences = (
      pc as unknown as { setCodecPreferences?: (list: RemoteDesktopCodecCapability[]) => void }
    ).setCodecPreferences;
    if (typeof setCodecPreferences !== "function") {
      return;
    }
    const h264 = codecs.filter((codec) => codec.mimeType.toLowerCase() === "video/h264");
    const vp8 = codecs.filter((codec) => codec.mimeType.toLowerCase() === "video/vp8");
    if (h264.length === 0 && vp8.length === 0) {
      return;
    }
    const preferred = new Set([...h264, ...vp8]);
    setCodecPreferences.call(pc, [
      ...h264,
      ...vp8,
      ...codecs.filter((codec) => !preferred.has(codec)),
    ]);
  } catch (error) {
    logger.warn(
      "[remote-desktop] setCodecPreferences 失败，回退默认编解码协商:",
      error instanceof Error ? error.message : String(error),
    );
  }
}
