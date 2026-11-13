/**
 * 生产者帧率回调（spec: docs/specs/remote-desktop-latency.md §2.1/§2.2）。
 *
 * WHY：自适应目前只调码率（`set-bitrate` 的 bps），fps 在会话建立时定型。带宽不够时
 * 降码率只会让每帧更糊、排队不变；真正能降端到端延迟的是**让被控少产生帧**。
 * 这里按解码端丢帧/RTT 采样，最多每 10s 把采集 fps 减半（最低 15），恢复时按同样
 * 滞回逐级翻倍，最终回到画质档位预设。
 *
 * 纯函数，时钟注入，单测见 packages/ui/test/remoteDesktopProducerRate.test.ts。
 */

export const MIN_PRODUCER_FPS = 15;
/** 单窗丢帧数达到该值视为解码端跟不上（3s 采样窗）。 */
export const PRODUCER_DEGRADE_DROPS = 3;
/** 恢复需要连续健康采样数（3s/拍 ⇒ 约 12s）。 */
export const PRODUCER_RECOVER_STREAK = 4;
/** 降/升档最小驻留（毫秒）：防在阈值附近振荡。 */
export const PRODUCER_DWELL_MS = 10_000;
/** 「健康」的 RTT 上界；null（未测得）不算健康。 */
export const PRODUCER_HEALTHY_RTT_MS = 45;

export interface ProducerRateState {
  /** 当前降档后的 fps；null = 跟随画质档位预设（未降档）。 */
  fps: number | null;
  /** 上次降/升档时刻（Date.now 口径）。 */
  lastAdjustAt: number | null;
  /** 连续健康采样计数。 */
  healthyStreak: number;
}

export const INITIAL_PRODUCER_RATE_STATE: ProducerRateState = {
  fps: null,
  lastAdjustAt: null,
  healthyStreak: 0,
};

export interface ProducerRateSample {
  /** 当前画质档位的预设 fps（60/60/30）。 */
  baseFps: number;
  /** 上一采样窗内解码端丢弃/重同步丢弃的帧数。 */
  droppedFrames: number;
  /** 最近一次 pong 的 RTT；未测得为 null。 */
  rttMs: number | null;
}

/**
 * 推进一步；`nextFps` 非 null 表示调用方应把 fps 随 `set-bitrate` 下发。
 * `nextFps` 可能等于 `baseFps`（恢复到底），表示解除降档。
 */
export function stepProducerRate(
  state: ProducerRateState,
  sample: ProducerRateSample,
  nowMs: number,
): { state: ProducerRateState; nextFps: number | null } {
  const baseFps = Math.max(1, Math.round(sample.baseFps));
  const minFps = Math.min(MIN_PRODUCER_FPS, baseFps);
  const current = Math.min(state.fps ?? baseFps, baseFps);

  if (sample.droppedFrames >= PRODUCER_DEGRADE_DROPS) {
    if (current <= minFps) {
      return { state: { ...state, healthyStreak: 0 }, nextFps: null };
    }
    if (state.lastAdjustAt !== null && nowMs - state.lastAdjustAt < PRODUCER_DWELL_MS) {
      return { state: { ...state, healthyStreak: 0 }, nextFps: null };
    }
    const next = Math.max(minFps, Math.floor(current / 2));
    if (next >= current) {
      return { state: { ...state, healthyStreak: 0 }, nextFps: null };
    }
    return { state: { fps: next, lastAdjustAt: nowMs, healthyStreak: 0 }, nextFps: next };
  }

  const healthy =
    sample.droppedFrames === 0 && sample.rttMs !== null && sample.rttMs <= PRODUCER_HEALTHY_RTT_MS;
  const healthyStreak = healthy ? state.healthyStreak + 1 : 0;
  if (state.fps === null || healthyStreak < PRODUCER_RECOVER_STREAK) {
    return { state: { ...state, healthyStreak }, nextFps: null };
  }
  if (state.lastAdjustAt !== null && nowMs - state.lastAdjustAt < PRODUCER_DWELL_MS) {
    return { state: { ...state, healthyStreak }, nextFps: null };
  }
  // 逐级翻倍恢复；到顶后回到「跟随预设」，后续不再下发 fps。
  const next = Math.min(baseFps, current * 2);
  return {
    state: { fps: next >= baseFps ? null : next, lastAdjustAt: nowMs, healthyStreak: 0 },
    nextFps: next,
  };
}
