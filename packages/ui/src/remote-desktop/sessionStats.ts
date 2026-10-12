/**
 * 会话状态条统计采集器（纯计数，单测见 packages/ui/test/remoteDesktopSessionStats.test.ts）。
 *
 * 旧媒体面（WebRTC）的 getStats 读数随 P4 直替清理一并移除，改为自有事件计数：
 * - fps：`onDecoded` 由原生媒体面每次绘制一帧时回调，`sample()` 返回距上次采样的解码帧数并清零
 *   （3s 轮询 ⇒ 读数即「近 3s 解码帧数 ≈ 帧率」；首次采样前无解码返回 0 而非 null——
 *   「画面未连」由上层区分：hook 只在会话进行中采样）；
 * - RTT：`onPong` 由控制通道 pong 回调（RTT = 收到时刻 - ping 时间戳），取最近一次；
 * - 帧龄：`onFrameAge` 由原生媒体面每绘制一帧回调（glass-to-glass 近似，被控采集 →
 *   本端绘制，时钟偏移由 pong 标定），`sample()` 返回窗口内中位数（采样后清窗）；
 * - 分辨率：`onConfigured` 由解码器首帧/换 SPS 重配时回调，取最近一次。
 *
 * 注意：`sample()` 有副作用（清零帧计数与帧龄窗口），只应由唯一的轮询消费者按固定节拍调用。
 */

export interface RemoteDesktopSessionStats {
  fps: number | null;
  rttMs: number | null;
  /** 帧龄中位数（近一次采样窗口）；无样本 null。 */
  frameAgeMs: number | null;
  width: number | null;
  height: number | null;
}

export const EMPTY_REMOTE_DESKTOP_SESSION_STATS: RemoteDesktopSessionStats = {
  fps: null,
  rttMs: null,
  frameAgeMs: null,
  width: null,
  height: null,
};

export interface NativeSessionStatsCollector {
  /** 原生媒体面每绘制一帧调用一次（fps 计数）。 */
  onDecoded(): void;
  /** 收到控制通道 pong 时调用；rttMs = 收到时刻 - ping 时间戳。 */
  onPong(rttMs: number): void;
  /** 原生媒体面每绘制一帧调用一次其帧龄（毫秒）；sample() 取窗口内中位数。 */
  onFrameAge(ageMs: number): void;
  /** 解码器首帧/换分辨率配置时调用（分辨率读数）。 */
  onConfigured(width: number, height: number): void;
  /** 按轮询节拍采样：fps = 间隔内解码帧数（采样后清零），帧龄 = 窗口中位数（采样后清窗），
   *  其余取最近一次值。 */
  sample(): RemoteDesktopSessionStats;
}

/** 中位数（偶数个取中位两数均值四舍五入）。 */
function median(values: number[]): number {
  const sorted = [...values].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 1
    ? sorted[mid]!
    : Math.round((sorted[mid - 1]! + sorted[mid]!) / 2);
}

export function createNativeSessionStatsCollector(): NativeSessionStatsCollector {
  let decodedSinceLastSample = 0;
  let lastRttMs: number | null = null;
  let frameAgesSinceLastSample: number[] = [];
  let lastWidth: number | null = null;
  let lastHeight: number | null = null;

  return {
    onDecoded() {
      decodedSinceLastSample += 1;
    },
    onPong(rttMs) {
      lastRttMs = rttMs;
    },
    onFrameAge(ageMs) {
      frameAgesSinceLastSample.push(ageMs);
    },
    onConfigured(width, height) {
      lastWidth = width;
      lastHeight = height;
    },
    sample() {
      // 快照并清零：下一次采样只统计新解码的帧（轮询间隔内的增量）。
      const fps = decodedSinceLastSample;
      decodedSinceLastSample = 0;
      const frameAgeMs =
        frameAgesSinceLastSample.length > 0 ? median(frameAgesSinceLastSample) : null;
      frameAgesSinceLastSample = [];
      return { fps, rttMs: lastRttMs, frameAgeMs, width: lastWidth, height: lastHeight };
    },
  };
}
