/**
 * 会话状态条统计采集器（纯计数，单测见 packages/ui/test/remoteDesktopSessionStats.test.ts）。
 *
 * 旧媒体面（WebRTC）的 getStats 读数随 P4 直替清理一并移除，改为自有事件计数：
 * - fps：`onDecoded` 由原生媒体面每次绘制一帧时回调，`sample()` 按「窗口内解码帧数 ÷
 *   实际经过秒数」折算成**每秒帧率**并清零（2026-11-13 修正：旧口径直接把 3s 计数当 fps，
 *   真机 80 实际约 27；见 docs/specs/remote-desktop-latency.md §2.6）。首窗从采集器创建
 *   时刻起算；首次采样前无解码返回 0 而非 null——「画面未连」由上层区分（hook 只在会话
 *   进行中采样）；
 * - RTT：`onPong` 由控制通道 pong 回调（RTT = 收到时刻 - ping 时间戳），取最近一次；
 * - 帧龄：`onFrameAge` 由原生媒体面每绘制一帧回调（glass-to-glass 近似，被控采集 →
 *   本端绘制，时钟偏移由 pong 标定），`sample()` 返回窗口内中位数（采样后清窗）；
 * - 分辨率：`onConfigured` 由解码器首帧/换 SPS 重配时回调，取最近一次；
 * - 码率：`onBytesReceived` 由媒体 WS 每条下行帧回调字节数，`sample()` 按两拍间隔折算
 *   kbps（近一拍平均；首拍无间隔基准返回 null）；采样后清零；
 * - 解码丢弃：`onFrameDropped`（解码队列门控丢 delta）计数，采样后清零——丢帧>0 时
 *   状态条显式提示（画面质量劣化的直接解释）；
 * - 解码队列：`onDecodeQueue` 每个被接受帧的 `decodeQueueSize`（取最近一次），
 *   作为「链路是否在追上」的即时读数；
 * - 冻结次数：`onFrozen` 会话内每次进入冻结（锁屏/失去访问）累计（跨采样不清零）。
 *
 * 注意：`sample()` 有副作用（清零帧/丢帧计数与帧龄、字节窗口），只应由唯一的轮询消费者按固定节拍调用。
 */

export interface RemoteDesktopSessionStats {
  fps: number | null;
  rttMs: number | null;
  /** 帧龄中位数（近一次采样窗口）；无样本 null。 */
  frameAgeMs: number | null;
  width: number | null;
  height: number | null;
  /** 近一拍平均收流码率（kbps）；首拍（无间隔基准）null。 */
  bitrateKbps: number | null;
  /** 近一拍解码队列门控丢弃帧数（采样后清零）。 */
  decodeDrops: number;
  /** 最近一次解码前的解码队列长度；未解码过 null。 */
  decodeQueue: number | null;
  /** 会话累计冻结次数（freeze 进入次数，累计不清零）。 */
  freezeCount: number;
}

export const EMPTY_REMOTE_DESKTOP_SESSION_STATS: RemoteDesktopSessionStats = {
  fps: null,
  rttMs: null,
  frameAgeMs: null,
  width: null,
  height: null,
  bitrateKbps: null,
  decodeDrops: 0,
  decodeQueue: null,
  freezeCount: 0,
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
  /** 媒体 WS 每条下行帧调用一次（字节数），sample() 折算近一拍平均码率。 */
  onBytesReceived(bytes: number): void;
  /** 解码队列门控丢弃一帧时调用（采样后清零的计数）。 */
  onFrameDropped(): void;
  /** 每个被接受帧解码前调用一次其解码队列长度（取最近一次）。 */
  onDecodeQueue(size: number): void;
  /** 会话进入冻结（freeze 信号）时调用一次；累计计数。 */
  onFrozen(): void;
  /** 按轮询节拍采样：fps/丢帧/码率为间隔增量（采样后清零），帧龄取窗口中位数，
   *  其余取最近一次值；冻结次数为累计值。 */
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

export function createNativeSessionStatsCollector(
  options: { now?: () => number } = {},
): NativeSessionStatsCollector {
  const now = options.now ?? (() => Date.now());
  // 首窗的分母：从创建时刻起算，避免第一次采样拿不到时间基准而报 0。
  const createdAtMs = now();
  let decodedSinceLastSample = 0;
  let lastRttMs: number | null = null;
  let frameAgesSinceLastSample: number[] = [];
  let lastWidth: number | null = null;
  let lastHeight: number | null = null;
  let bytesSinceLastSample = 0;
  let lastSampleAtMs: number | null = null;
  let dropsSinceLastSample = 0;
  let lastDecodeQueue: number | null = null;
  let freezeCount = 0;

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
    onBytesReceived(bytes) {
      bytesSinceLastSample += bytes;
    },
    onFrameDropped() {
      dropsSinceLastSample += 1;
    },
    onDecodeQueue(size) {
      lastDecodeQueue = size;
    },
    onFrozen() {
      freezeCount += 1;
    },
    sample() {
      const sampleAtMs = now();
      // fps 是速率而不是计数：3s 窗口内 60 帧应显示 20，不是 60。
      const elapsedMs = sampleAtMs - (lastSampleAtMs ?? createdAtMs);
      const fps =
        elapsedMs > 0 ? Math.round((decodedSinceLastSample * 1000) / elapsedMs) : 0;
      decodedSinceLastSample = 0;
      const frameAgeMs =
        frameAgesSinceLastSample.length > 0 ? median(frameAgesSinceLastSample) : null;
      frameAgesSinceLastSample = [];
      // 码率 = 字节 × 8 / 间隔秒；首拍（无间隔基准）没有可用的时间分母 → null。
      const intervalMs = lastSampleAtMs === null ? 0 : sampleAtMs - lastSampleAtMs;
      const bitrateKbps =
        intervalMs > 0 ? Math.round((bytesSinceLastSample * 8) / intervalMs) : null;
      bytesSinceLastSample = 0;
      lastSampleAtMs = sampleAtMs;
      const decodeDrops = dropsSinceLastSample;
      dropsSinceLastSample = 0;
      return {
        fps,
        rttMs: lastRttMs,
        frameAgeMs,
        width: lastWidth,
        height: lastHeight,
        bitrateKbps,
        decodeDrops,
        decodeQueue: lastDecodeQueue,
        freezeCount,
      };
    },
  };
}
