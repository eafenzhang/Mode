//! 会话状态条统计采集器（纯计数）。
//!
//! 真源：`packages/ui/src/remote-desktop/sessionStats.ts`。
//! 不读时钟：TS 侧构造时注入 `now`，Rust 侧改为在 `sample(now_ms)` 传参，
//! 可观测行为一致（时间戳只在采样时消费）。状态条空态常量
//! `EMPTY_REMOTE_DESKTOP_SESSION_STATS` 是渲染层展示默认值，不随采集器迁移。

use serde_json::{json, Value};

/// 中位数：奇数个取中位；偶数个取中位两数均值四舍五入（JS `Math.round` 半值向上）。
fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        sorted[mid]
    } else {
        ((sorted[mid - 1] + sorted[mid]) / 2.0).round()
    }
}

/// JSON 数值输出与 TS 对齐：整数值发整数（`17` 而非 `17.0`），小数保留。
fn num_to_json(value: f64) -> Value {
    if value.fract() == 0.0 && value.is_finite() && value.abs() <= i64::MAX as f64 {
        json!(value as i64)
    } else {
        json!(value)
    }
}

fn option_num_to_json(value: Option<f64>) -> Value {
    match value {
        Some(value) => num_to_json(value),
        None => Value::Null,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionStats {
    pub fps: i64,
    pub rtt_ms: Option<f64>,
    /// 帧龄中位数（近一次采样窗口）；无样本 null。
    pub frame_age_ms: Option<f64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    /// 近一拍平均收流码率（kbps）；首拍（无间隔基准）null。
    pub bitrate_kbps: Option<f64>,
    /// 近一拍解码队列门控丢弃帧数（采样后清零）。
    pub decode_drops: i64,
    /// 最近一次解码前的解码队列长度；未解码过 null。
    pub decode_queue: Option<f64>,
    /// 会话累计冻结次数（freeze 进入次数，累计不清零）。
    pub freeze_count: i64,
}

impl SessionStats {
    pub fn to_json(&self) -> Value {
        json!({
            "fps": self.fps,
            "rttMs": option_num_to_json(self.rtt_ms),
            "frameAgeMs": option_num_to_json(self.frame_age_ms),
            "width": self.width,
            "height": self.height,
            "bitrateKbps": option_num_to_json(self.bitrate_kbps),
            "decodeDrops": self.decode_drops,
            "decodeQueue": option_num_to_json(self.decode_queue),
            "freezeCount": self.freeze_count,
        })
    }
}

#[derive(Debug, Default)]
pub struct SessionStatsCollector {
    decoded_since_last_sample: i64,
    last_rtt_ms: Option<f64>,
    frame_ages_since_last_sample: Vec<f64>,
    last_width: Option<i64>,
    last_height: Option<i64>,
    bytes_since_last_sample: f64,
    last_sample_at_ms: Option<f64>,
    drops_since_last_sample: i64,
    last_decode_queue: Option<f64>,
    freeze_count: i64,
}

impl SessionStatsCollector {
    pub fn new() -> Self {
        Self::default()
    }

    /// 原生媒体面每绘制一帧调用一次（fps 计数）。
    pub fn on_decoded(&mut self) {
        self.decoded_since_last_sample += 1;
    }

    /// 收到控制通道 pong 时调用；rtt 由调用方按「收到时刻 - ping 时间戳」算好传入。
    pub fn on_pong(&mut self, rtt_ms: f64) {
        self.last_rtt_ms = Some(rtt_ms);
    }

    /// 原生媒体面每绘制一帧调用一次其帧龄（毫秒）；sample() 取窗口内中位数。
    pub fn on_frame_age(&mut self, age_ms: f64) {
        self.frame_ages_since_last_sample.push(age_ms);
    }

    /// 解码器首帧/换分辨率配置时调用（分辨率读数取最近一次）。
    pub fn on_configured(&mut self, width: i64, height: i64) {
        self.last_width = Some(width);
        self.last_height = Some(height);
    }

    /// 媒体 WS 每条下行帧调用一次（字节数），sample() 折算近一拍平均码率。
    pub fn on_bytes_received(&mut self, bytes: f64) {
        self.bytes_since_last_sample += bytes;
    }

    /// 解码队列门控丢弃一帧时调用（采样后清零的计数）。
    pub fn on_frame_dropped(&mut self) {
        self.drops_since_last_sample += 1;
    }

    /// 每个被接受帧解码前调用一次其解码队列长度（取最近一次）。
    pub fn on_decode_queue(&mut self, size: f64) {
        self.last_decode_queue = Some(size);
    }

    /// 会话进入冻结（freeze 信号）时调用一次；累计计数，跨采样不清零。
    pub fn on_frozen(&mut self) {
        self.freeze_count += 1;
    }

    /// 按轮询节拍采样：fps/丢帧/码率为间隔增量（采样后清零），帧龄取窗口中位数，
    /// 其余取最近一次值；冻结次数为累计值。
    ///
    /// 有副作用（清零计数与窗口），只能由唯一的轮询消费者按固定节拍调用。
    pub fn sample(&mut self, now_ms: f64) -> SessionStats {
        let fps = self.decoded_since_last_sample;
        self.decoded_since_last_sample = 0;
        let frame_age_ms = if self.frame_ages_since_last_sample.is_empty() {
            None
        } else {
            Some(median(&self.frame_ages_since_last_sample))
        };
        self.frame_ages_since_last_sample.clear();
        // 码率 = 字节 × 8 / 间隔秒；首拍（无间隔基准）没有可用的时间分母 → null。
        let interval_ms = match self.last_sample_at_ms {
            None => 0.0,
            Some(last) => now_ms - last,
        };
        let bitrate_kbps = if interval_ms > 0.0 {
            Some((self.bytes_since_last_sample * 8.0 / interval_ms).round())
        } else {
            None
        };
        self.bytes_since_last_sample = 0.0;
        self.last_sample_at_ms = Some(now_ms);
        let decode_drops = self.drops_since_last_sample;
        self.drops_since_last_sample = 0;
        SessionStats {
            fps,
            rtt_ms: self.last_rtt_ms,
            frame_age_ms,
            width: self.last_width,
            height: self.last_height,
            bitrate_kbps,
            decode_drops,
            decode_queue: self.last_decode_queue,
            freeze_count: self.freeze_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fps_counts_within_window_and_resets() {
        let mut c = SessionStatsCollector::new();
        for _ in 0..60 {
            c.on_decoded();
        }
        assert_eq!(c.sample(0.0).fps, 60);
        assert_eq!(c.sample(0.0).fps, 0);
    }

    #[test]
    fn rtt_and_resolution_track_latest() {
        let mut c = SessionStatsCollector::new();
        c.on_pong(17.0);
        c.on_pong(19.0);
        c.on_configured(1920, 1080);
        let stats = c.sample(0.0);
        assert_eq!(stats.rtt_ms, Some(19.0));
        assert_eq!(stats.width, Some(1920));
        assert_eq!(stats.height, Some(1080));
    }

    #[test]
    fn frame_age_takes_window_median_then_clears() {
        let mut c = SessionStatsCollector::new();
        assert_eq!(c.sample(0.0).frame_age_ms, None);
        for age in [30.0, 90.0, 20.0] {
            c.on_frame_age(age);
        }
        assert_eq!(c.sample(0.0).frame_age_ms, Some(30.0));
        c.on_frame_age(10.0);
        c.on_frame_age(21.0);
        // 偶数个取中位两数均值四舍五入（15.5 → 16）。
        assert_eq!(c.sample(0.0).frame_age_ms, Some(16.0));
        assert_eq!(c.sample(0.0).frame_age_ms, None);
    }

    #[test]
    fn bitrate_first_sample_has_no_denominator_then_resets() {
        let mut c = SessionStatsCollector::new();
        c.on_bytes_received(999.0);
        assert_eq!(c.sample(1_000_000.0).bitrate_kbps, None);
        c.on_bytes_received(3_000_000.0);
        assert_eq!(c.sample(1_003_000.0).bitrate_kbps, Some(8000.0));
        // 窗口清零：下一拍无新字节 → 0 kbps（而非残留旧值）。
        assert_eq!(c.sample(1_006_000.0).bitrate_kbps, Some(0.0));
    }

    #[test]
    fn drops_reset_and_queue_and_freeze_track_latest_or_cumulative() {
        let mut c = SessionStatsCollector::new();
        for _ in 0..3 {
            c.on_frame_dropped();
        }
        assert_eq!(c.sample(0.0).decode_drops, 3);
        assert_eq!(c.sample(0.0).decode_drops, 0);
        assert_eq!(c.sample(0.0).decode_queue, None);
        c.on_decode_queue(1.0);
        c.on_decode_queue(4.0);
        assert_eq!(c.sample(0.0).decode_queue, Some(4.0));
        c.on_frozen();
        assert_eq!(c.sample(0.0).freeze_count, 1);
        c.on_frozen();
        assert_eq!(c.sample(0.0).freeze_count, 2);
        assert_eq!(c.sample(0.0).freeze_count, 2);
    }
}
