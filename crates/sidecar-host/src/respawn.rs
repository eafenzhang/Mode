//! Host 自动重生的退避策略（**纯策略**，不碰进程；由 `mode-tauri` 的监督线程调用）。
//!
//! 口径（spec `tauri-renderer-host.md` §22.4）：
//!
//! - 只有**进程事实**触发生（`HostHandle::wait_for_exit`），渲染层看到的状态不参与决策；
//! - 连续「快速崩溃」才升级退避：距上次成功起进程不足 [`QUICK_CRASH_WINDOW`] 记一次；
//!   活得够久才死的，视为新一轮（记 1 次、立即重生）；
//! - [`MAX_QUICK_CRASHES`] 次快速崩溃后**放弃自动重生**（`None`），留给人工重试——
//!   无上限重启会变成崩溃循环，把「Host 起不来」伪装成「应用卡在加载」。
//!
//! 为什么策略放这里：本 crate 的测试目标在本机 gnu 路线能真跑（`mode-tauri` 不能），
//! 与 `settings_probe` / `pick_entry` 同一种分工。

use std::time::{Duration, Instant};

/// 距上次起进程不足这个时长就死亡 = 一次「快速崩溃」。
///
/// 2026-10-27 由一个真 bug 修正：窗口曾设 60s，而心跳误杀循环的间隔恰好 ~61s——
/// 每一次都被当成「活得够久的新一轮崩溃」，退避上限永远不触发，Host 无限重生。
/// 300s 覆盖「起来没多久又被弄死」的所有形态（含心跳/外部杀的循环），代价只有
/// 「活过 5 分钟才死」的偶发崩溃也会多计一次预算——可接受。
pub const QUICK_CRASH_WINDOW: Duration = Duration::from_secs(300);

/// 快速崩溃的容忍次数（含第一次）。超了就放弃自动重生。
pub const MAX_QUICK_CRASHES: u32 = 3;

/// 每次快速崩溃后的等待：第一次立即重试，之后 2s、10s。
pub const BACKOFF: [Duration; MAX_QUICK_CRASHES as usize] = [
    Duration::ZERO,
    Duration::from_secs(2),
    Duration::from_secs(10),
];

/// 一个监督周期内的重生历史。`Default` = 全清零（首次启动前/人工重试后）。
#[derive(Debug, Default)]
pub struct RespawnHistory {
    last_spawn: Option<Instant>,
    quick_crashes: u32,
}

impl RespawnHistory {
    /// 每成功/失败地拉起一代都记一次（失败的重生同样计入，避免「起不来 → 立刻再起」空转）。
    pub fn record_spawn(&mut self, now: Instant) {
        self.last_spawn = Some(now);
    }

    /// 记录一次死亡，返回下次重生前的等待；`None` = 放弃自动重生（等人工 retry）。
    ///
    /// 「快速」判据用**距上次起进程**的时长，而不是「距上次死亡」：只有反复起不来才是
    /// 崩溃循环；稳定运行一小时后偶发一次死亡，值得立刻重启。
    pub fn record_death(&mut self, now: Instant) -> Option<Duration> {
        let quick = self
            .last_spawn
            .map(|started| now.duration_since(started) < QUICK_CRASH_WINDOW)
            // 还没记过 spawn 就死了（首启即失败？）：按快速崩溃处理，宁可保守。
            .unwrap_or(true);
        self.quick_crashes = if quick { self.quick_crashes + 1 } else { 1 };
        if self.quick_crashes > MAX_QUICK_CRASHES {
            return None;
        }
        Some(BACKOFF[(self.quick_crashes - 1) as usize])
    }

    /// 清空历史（人工重试：给一整轮新的重试预算）。
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn quick_crashes(&self) -> u32 {
        self.quick_crashes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, seconds: u64) -> Instant {
        start + Duration::from_secs(seconds)
    }

    #[test]
    fn quick_crashes_escalate_then_give_up() {
        let start = Instant::now();
        let mut history = RespawnHistory::default();
        history.record_spawn(start);
        assert_eq!(history.record_death(at(start, 1)), Some(Duration::ZERO));
        assert_eq!(
            history.record_death(at(start, 2)),
            Some(Duration::from_secs(2))
        );
        assert_eq!(
            history.record_death(at(start, 3)),
            Some(Duration::from_secs(10))
        );
        assert_eq!(
            history.record_death(at(start, 4)),
            None,
            "第四次快速崩溃应放弃"
        );
        assert_eq!(history.quick_crashes(), 4);
    }

    #[test]
    fn a_kill_loop_at_61s_still_counts_as_quick_crash() {
        // 61s 恰好越过旧窗口 60s 的真实杀戳间隔：必须按快速崩溃计，否则上限失效。
        let start = Instant::now();
        let mut history = RespawnHistory::default();
        history.record_spawn(start);
        assert_eq!(history.record_death(at(start, 61)), Some(Duration::ZERO));
        assert_eq!(history.record_death(at(start, 122)), Some(Duration::from_secs(2)));
        assert_eq!(history.record_death(at(start, 183)), Some(Duration::from_secs(10)));
        assert_eq!(history.record_death(at(start, 244)), None, "第四次 61s 间隔必须放弃");
    }

    #[test]
    fn a_long_lived_host_resets_the_budget() {
        let start = Instant::now();
        let mut history = RespawnHistory::default();
        history.record_spawn(start);
        // 第一次快速崩溃 ×2。
        assert_eq!(history.record_death(at(start, 1)), Some(Duration::ZERO));
        assert_eq!(
            history.record_death(at(start, 2)),
            Some(Duration::from_secs(2))
        );
        // 重生后活了 10 分钟：预算回到 1，立即重生。
        history.record_spawn(at(start, 3));
        assert_eq!(
            history.record_death(at(start, 3 + 600)),
            Some(Duration::ZERO)
        );
        assert_eq!(history.quick_crashes(), 1);
    }

    #[test]
    fn reset_restores_the_full_budget() {
        let start = Instant::now();
        let mut history = RespawnHistory::default();
        history.record_spawn(start);
        for second in 0..3 {
            assert!(history.record_death(at(start, second)).is_some());
        }
        assert_eq!(history.record_death(at(start, 9)), None);
        history.reset();
        history.record_spawn(at(start, 10));
        assert_eq!(history.record_death(at(start, 11)), Some(Duration::ZERO));
    }

    #[test]
    fn death_before_any_spawn_counts_as_quick() {
        let mut history = RespawnHistory::default();
        assert_eq!(history.record_death(Instant::now()), Some(Duration::ZERO));
        assert_eq!(history.quick_crashes(), 1);
    }
}
