/**
 * 自适应码率 v1（纯逻辑；测试见 remoteDesktopAdaptiveBitrate.test.ts）。
 *
 * 判定输入（渲染侧唯一可信信号，无需跨端时钟）：控制通道 ping/pong 量出的 RTT、
 * 上一窗口的解码丢弃数（解码队列门控 + 送达丢弃）。决策输出经 media WS 下行
 * `{t:"set-bitrate", bps}` → 中继 → helper → `ICodecAPI` 直接改编码器 MeanBitRate。
 *
 * 策略（两档 + 非对称滞回）：
 * - 降档快：RTT 超阈或出现任何丢弃 → 立即降到低档（拥塞时先稳住延迟，画质让位）；
 * - 升档慢：低档下连续 `RECOVER_STREAK` 次健康采样且距上次切换 ≥ `MIN_DWELL_MS`
 *   才升档（防抖振——链路在阈值附近来回时停留在低档）。
 */

export const ADAPTIVE_BITRATE_HIGH = 8_000_000;
export const ADAPTIVE_BITRATE_LOW = 4_000_000;

/** 降档 RTT 阈值（LAN 正常 <20ms；60ms 已明显偏离基线）。 */
const DEGRADE_RTT_MS = 60;
/** 恢复要求「健康」为新阈一半：留出滞回带，不在降档线附近抖动。 */
const RECOVER_RTT_MS = DEGRADE_RTT_MS / 2;
/** 恢复所需连续健康采样次数（ping 节拍 3s ⇒ 约 12s 稳定后回升）。 */
const RECOVER_STREAK = 4;
/** 最小驻留（毫秒）：上次切换后至少保持这么久才允许升档。 */
const MIN_DWELL_MS = 10_000;

export interface AdaptiveBitrateSample {
  /** 最近一次 pong 的 RTT；未测得为 null（不算健康，也不算拥塞）。 */
  rttMs: number | null;
  /** 自上次采样以来被丢弃的帧数（解码队列门控丢弃等）。 */
  droppedFrames: number;
}

export interface AdaptiveBitrateState {
  currentBps: number;
  /** 本会话高档码率（P1-3：由画质档位预设决定；缺省 = 旧常量）。 */
  highBps: number;
  /** 本会话低档码率（拥塞时降到这一档，不再继续降）。 */
  lowBps: number;
  /** 上次切换发生时刻（Date.now 口径）；未切换过为 null。 */
  lastSwitchAt: number | null;
  /** 连续健康采样计数（任一采样不健康即清零）。 */
  healthyStreak: number;
}

/** 初始为高档。`highBps`/`lowBps` 由调用方按画质档位传入（P1-3）；缺省保留旧两档常量。 */
export function initialAdaptiveBitrateState(
  highBps: number = ADAPTIVE_BITRATE_HIGH,
  lowBps: number = ADAPTIVE_BITRATE_LOW,
): AdaptiveBitrateState {
  return {
    currentBps: highBps,
    highBps,
    // 低档不得高于高档（畸形入参按高档收敛，避免升/降档语义反转）。
    lowBps: Math.min(lowBps, highBps),
    lastSwitchAt: null,
    healthyStreak: 0,
  };
}

/**
 * 推进一步自适应判定；`nextBps` 非 null 表示调用方应下发一次 set-bitrate。
 * `nowMs` 注入 Date.now() 口径（可测试）。
 */
export function stepAdaptiveBitrate(
  state: AdaptiveBitrateState,
  sample: AdaptiveBitrateSample,
  nowMs: number,
): { state: AdaptiveBitrateState; nextBps: number | null } {
  const congested =
    (sample.rttMs !== null && sample.rttMs > DEGRADE_RTT_MS) || sample.droppedFrames > 0;
  if (congested) {
    if (state.currentBps === state.lowBps) {
      return { state: { ...state, healthyStreak: 0 }, nextBps: null }; // 已在低档，无需重复下发
    }
    return {
      state: { ...state, currentBps: state.lowBps, lastSwitchAt: nowMs, healthyStreak: 0 },
      nextBps: state.lowBps,
    };
  }
  const healthy = sample.rttMs !== null && sample.rttMs <= RECOVER_RTT_MS;
  const healthyStreak = healthy ? state.healthyStreak + 1 : 0;
  if (
    state.currentBps === state.lowBps &&
    healthyStreak >= RECOVER_STREAK &&
    (state.lastSwitchAt === null || nowMs - state.lastSwitchAt >= MIN_DWELL_MS)
  ) {
    return {
      state: { ...state, currentBps: state.highBps, lastSwitchAt: nowMs, healthyStreak: 0 },
      nextBps: state.highBps,
    };
  }
  return { state: { ...state, healthyStreak }, nextBps: null };
}
