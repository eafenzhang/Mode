/**
 * 会话状态条统计采集器（纯计数，单测见 packages/ui/test/remoteDesktopSessionStats.test.ts）。
 *
 * 原实现读 `RTCPeerConnection.getStats()`；renderer 切原生媒体面（WebCodecs + media WS）后，
 * 这些 RTC 读数不再存在，改为自有事件计数：
 * - fps：`onDecoded` 由原生媒体面每次绘制一帧时回调，`sample()` 返回距上次采样的解码帧数并清零
 *   （3s 轮询 ⇒ 读数即「近 3s 解码帧数 ≈ 帧率」；首次采样前无解码返回 0 而非 null——
 *   「画面未连」由上层区分：hook 只在会话进行中采样）；
 * - RTT：`onPong` 由控制通道 pong 回调（RTT = 收到时刻 - ping 时间戳），取最近一次；
 * - 分辨率：`onConfigured` 由解码器首帧/换 SPS 重配时回调，取最近一次。
 *
 * 注意：`sample()` 有副作用（清零帧计数），只应由唯一的轮询消费者按固定节拍调用。
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

export interface NativeSessionStatsCollector {
  /** 原生媒体面每绘制一帧调用一次（fps 计数）。 */
  onDecoded(): void;
  /** 收到控制通道 pong 时调用；rttMs = 收到时刻 - ping 时间戳。 */
  onPong(rttMs: number): void;
  /** 解码器首帧/换分辨率配置时调用（分辨率读数）。 */
  onConfigured(width: number, height: number): void;
  /** 按轮询节拍采样：fps = 间隔内解码帧数（采样后清零），其余取最近一次值。 */
  sample(): RemoteDesktopSessionStats;
}

export function createNativeSessionStatsCollector(): NativeSessionStatsCollector {
  let decodedSinceLastSample = 0;
  let lastRttMs: number | null = null;
  let lastWidth: number | null = null;
  let lastHeight: number | null = null;

  return {
    onDecoded() {
      decodedSinceLastSample += 1;
    },
    onPong(rttMs) {
      lastRttMs = rttMs;
    },
    onConfigured(width, height) {
      lastWidth = width;
      lastHeight = height;
    },
    sample() {
      // 快照并清零：下一次采样只统计新解码的帧（轮询间隔内的增量）。
      const fps = decodedSinceLastSample;
      decodedSinceLastSample = 0;
      return { fps, rttMs: lastRttMs, width: lastWidth, height: lastHeight };
    },
  };
}
