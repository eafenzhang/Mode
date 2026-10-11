/**
 * 会话状态条统计（纯函数，单测见 packages/ui/test/remoteDesktopSessionStats.test.ts）。
 *
 * 数据源是 `RTCPeerConnection.getStats()`（3s 轮询，spec UI 规则）：
 * - fps：优先 inbound-rtp 的 `framesPerSecond`（部分版本缺失时用 framesDecoded 差分）；
 * - RTT：已选中（nominated）且成功的 candidate-pair 的 `currentRoundTripTime`（单位秒）；
 * - 分辨率：inbound-rtp 的 frameWidth/frameHeight（无则留空，界面回退到显示器源尺寸）。
 */

export interface RemoteDesktopSessionStats {
  fps: number | null;
  rttMs: number | null;
  width: number | null;
  height: number | null;
}

export const EMPTY_REMOTE_DESKTOP_SESSION_STATS: RemoteDesktopSessionStats = {
  fps: null,
  rttMs: null,
  width: null,
  height: null,
};

/** 上一轮解码帧数快照（fps 差分的参照；首轮无参照时 fps 留空）。 */
export interface RemoteDesktopFpsBaseline {
  frames: number;
  at: number;
}

export interface RemoteDesktopSessionStatsSample {
  stats: RemoteDesktopSessionStats;
  baseline: RemoteDesktopFpsBaseline | null;
}

function resolveFps(
  inbound: RTCInboundRtpStreamStats,
  baseline: RemoteDesktopFpsBaseline | null,
): { fps: number | null; baseline: RemoteDesktopFpsBaseline } {
  const at = inbound.timestamp;
  const frames = inbound.framesDecoded ?? 0;
  if (typeof inbound.framesPerSecond === "number") {
    return { fps: Math.max(0, Math.round(inbound.framesPerSecond)), baseline: { frames, at } };
  }
  if (!baseline || !(at > baseline.at)) {
    return { fps: null, baseline: { frames, at } };
  }
  const elapsedMs = at - baseline.at;
  const deltaFrames = frames - baseline.frames;
  return {
    fps: Math.max(0, Math.round((deltaFrames * 1000) / elapsedMs)),
    baseline: { frames, at },
  };
}

/** 单次 getStats 报告 → 状态条读数；多路视频/候选时取最后一条成功读数。 */
export function deriveRemoteDesktopSessionStats(
  report: RTCStatsReport,
  baseline: RemoteDesktopFpsBaseline | null,
): RemoteDesktopSessionStatsSample {
  let stats = EMPTY_REMOTE_DESKTOP_SESSION_STATS;
  let nextBaseline = baseline;
  for (const entry of report.values()) {
    if (entry.type === "inbound-rtp" && "kind" in entry && entry.kind === "video") {
      const inbound = entry as RTCInboundRtpStreamStats;
      const fps = resolveFps(inbound, nextBaseline);
      nextBaseline = fps.baseline;
      stats = {
        ...stats,
        fps: fps.fps,
        width: inbound.frameWidth ?? stats.width,
        height: inbound.frameHeight ?? stats.height,
      };
      continue;
    }
    if (entry.type === "candidate-pair" && "state" in entry && entry.state === "succeeded") {
      const pair = entry as RTCIceCandidatePairStats;
      // RTT 只取已选中的成功配对（可能同时存在多条成功配对的历史记录）。
      if (typeof pair.currentRoundTripTime === "number" && pair.nominated !== false) {
        stats = { ...stats, rttMs: Math.round(pair.currentRoundTripTime * 1000) };
      }
    }
  }
  return { stats, baseline: nextBaseline };
}
