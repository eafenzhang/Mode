//! 远程桌面自适应码率与画质降档纯状态机。
//!
//! 真源：`packages/ui/src/remote-desktop/adaptiveBitrate.ts`。
//! 不发网络、不读时钟：`nowMs` 与采样由调用方注入。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const ADAPTIVE_BITRATE_HIGH: i64 = 8_000_000;
pub const ADAPTIVE_BITRATE_LOW: i64 = 4_000_000;
const DEGRADE_RTT_MS: f64 = 60.0;
const RECOVER_RTT_MS: f64 = DEGRADE_RTT_MS / 2.0;
const RECOVER_STREAK: i64 = 4;
const MIN_DWELL_MS: i64 = 10_000;

pub const DOWNSHIFT_DROPPED_FRAMES: i64 = 3;
pub const DOWNSHIFT_RTT_MS: f64 = 120.0;
pub const DOWNSHIFT_STREAK: i64 = 2;
pub const DOWNSHIFT_COOLDOWN_MS: i64 = 30_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RemoteDesktopQuality {
    Sharp,
    Balanced,
    Smooth,
}

impl RemoteDesktopQuality {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sharp => "sharp",
            Self::Balanced => "balanced",
            Self::Smooth => "smooth",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "sharp" => Some(Self::Sharp),
            "balanced" => Some(Self::Balanced),
            "smooth" => Some(Self::Smooth),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AdaptiveBitrateSample {
    pub rtt_ms: Option<f64>,
    pub dropped_frames: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdaptiveBitrateState {
    pub current_bps: i64,
    pub high_bps: i64,
    pub low_bps: i64,
    pub last_switch_at: Option<i64>,
    pub healthy_streak: i64,
}

pub fn initial_adaptive_bitrate_state(high_bps: i64, low_bps: i64) -> AdaptiveBitrateState {
    AdaptiveBitrateState {
        current_bps: high_bps,
        high_bps,
        // 低档不得高于高档（畸形入参按高档收敛，避免升/降档语义反转）。
        low_bps: low_bps.min(high_bps),
        last_switch_at: None,
        healthy_streak: 0,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdaptiveBitrateStep {
    pub state: AdaptiveBitrateState,
    pub next_bps: Option<i64>,
}

pub fn step_adaptive_bitrate(
    state: &AdaptiveBitrateState,
    sample: &AdaptiveBitrateSample,
    now_ms: i64,
) -> AdaptiveBitrateStep {
    let congested =
        sample.rtt_ms.is_some_and(|rtt| rtt > DEGRADE_RTT_MS) || sample.dropped_frames > 0;
    if congested {
        if state.current_bps == state.low_bps {
            return AdaptiveBitrateStep {
                state: AdaptiveBitrateState {
                    healthy_streak: 0,
                    ..state.clone()
                },
                next_bps: None,
            };
        }
        return AdaptiveBitrateStep {
            state: AdaptiveBitrateState {
                current_bps: state.low_bps,
                last_switch_at: Some(now_ms),
                healthy_streak: 0,
                ..state.clone()
            },
            next_bps: Some(state.low_bps),
        };
    }
    let healthy = sample.rtt_ms.is_some_and(|rtt| rtt <= RECOVER_RTT_MS);
    let healthy_streak = if healthy { state.healthy_streak + 1 } else { 0 };
    let cooled_down = state
        .last_switch_at
        .is_none_or(|last| now_ms - last >= MIN_DWELL_MS);
    if state.current_bps == state.low_bps && healthy_streak >= RECOVER_STREAK && cooled_down {
        return AdaptiveBitrateStep {
            state: AdaptiveBitrateState {
                current_bps: state.high_bps,
                last_switch_at: Some(now_ms),
                healthy_streak: 0,
                ..state.clone()
            },
            next_bps: Some(state.high_bps),
        };
    }
    AdaptiveBitrateStep {
        state: AdaptiveBitrateState {
            healthy_streak,
            ..state.clone()
        },
        next_bps: None,
    }
}

pub fn next_lower_quality(quality: RemoteDesktopQuality) -> Option<RemoteDesktopQuality> {
    match quality {
        RemoteDesktopQuality::Sharp => Some(RemoteDesktopQuality::Balanced),
        RemoteDesktopQuality::Balanced => Some(RemoteDesktopQuality::Smooth),
        RemoteDesktopQuality::Smooth => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QualityDownshiftState {
    pub congested_streak: i64,
    pub last_downshift_at: Option<i64>,
}

pub const INITIAL_QUALITY_DOWNSHIFT_STATE: QualityDownshiftState = QualityDownshiftState {
    congested_streak: 0,
    last_downshift_at: None,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QualityDownshiftStep {
    pub state: QualityDownshiftState,
    pub downshift: bool,
}

pub fn step_quality_downshift(
    state: &QualityDownshiftState,
    sample: &AdaptiveBitrateSample,
    now_ms: i64,
) -> QualityDownshiftStep {
    let congested = sample.dropped_frames >= DOWNSHIFT_DROPPED_FRAMES
        && sample.rtt_ms.is_some_and(|rtt| rtt >= DOWNSHIFT_RTT_MS);
    if !congested {
        return QualityDownshiftStep {
            state: QualityDownshiftState {
                congested_streak: 0,
                ..*state
            },
            downshift: false,
        };
    }
    let congested_streak = state.congested_streak + 1;
    let cooled_down = state
        .last_downshift_at
        .is_none_or(|last| now_ms - last >= DOWNSHIFT_COOLDOWN_MS);
    if congested_streak >= DOWNSHIFT_STREAK && cooled_down {
        return QualityDownshiftStep {
            state: QualityDownshiftState {
                congested_streak: 0,
                last_downshift_at: Some(now_ms),
            },
            downshift: true,
        };
    }
    QualityDownshiftStep {
        state: QualityDownshiftState {
            congested_streak,
            ..*state
        },
        downshift: false,
    }
}

pub fn adaptive_bitrate_state_to_json(state: &AdaptiveBitrateState) -> Value {
    json!({
        "currentBps": state.current_bps,
        "highBps": state.high_bps,
        "lowBps": state.low_bps,
        "lastSwitchAt": state.last_switch_at,
        "healthyStreak": state.healthy_streak,
    })
}

pub fn adaptive_bitrate_step_to_json(step: &AdaptiveBitrateStep) -> Value {
    json!({
        "state": adaptive_bitrate_state_to_json(&step.state),
        "nextBps": step.next_bps,
    })
}

pub fn quality_downshift_state_to_json(state: &QualityDownshiftState) -> Value {
    json!({
        "congestedStreak": state.congested_streak,
        "lastDownshiftAt": state.last_downshift_at,
    })
}

pub fn quality_downshift_step_to_json(step: &QualityDownshiftStep) -> Value {
    json!({
        "state": quality_downshift_state_to_json(&step.state),
        "downshift": step.downshift,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn degrades_then_recovers_after_streak_and_dwell() {
        let state = initial_adaptive_bitrate_state(8_000_000, 4_000_000);
        let sample = AdaptiveBitrateSample {
            rtt_ms: Some(80.0),
            dropped_frames: 0,
        };
        let degraded = step_adaptive_bitrate(&state, &sample, 1_000);
        assert_eq!(degraded.next_bps, Some(4_000_000));
        let mut state = degraded.state;
        let healthy = AdaptiveBitrateSample {
            rtt_ms: Some(20.0),
            dropped_frames: 0,
        };
        for _ in 0..3 {
            let step = step_adaptive_bitrate(&state, &healthy, 10_000);
            assert_eq!(step.next_bps, None);
            state = step.state;
        }
        let recovered = step_adaptive_bitrate(&state, &healthy, 15_000);
        assert_eq!(recovered.next_bps, Some(8_000_000));
    }
}
