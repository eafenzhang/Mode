//! Host 的 **stderr** 一侧：留痕 + 抽取结构化失败事实（DR-T3-3 ①）。
//!
//! ## 为什么需要这条通道
//!
//! Host 在真启动里失败时会结构化落一行 stderr 并退出：
//!
//! ```text
//! [mode-host-sidecar] startup_failed phase=failed failedPhase=preparing_host_storage errorCode=sql_failed error=ENOTDIR: …
//! ```
//!
//! 它同时还**不写任何帧**、以非零码退出——所以在壳这一侧，那次退出看起来与「Host 随便崩了」
//! 一模一样，只能报 `transport_closed` / `failedPhase=starting`。渲染层于是拿到一个比事实粗的原因：
//! Host 知道的是「准备任务索引时存储打开失败」，渲染层看到的是「就绪前就没了」。
//!
//! 这个差额不只是措辞：`canRetryDatabaseStartup`（`packages/shared/src/database-startup.ts`）
//! 按 `failedPhase` + `errorCode` 决定要不要给用户「重试」——`transport_closed` 不在可重试集合里，
//! 而 `sql_failed` + `preparing_host_storage` 在。粗粒度原因会把一次**该给重试按钮**的失败
//! 变成只有「退出」可选。
//!
//! ## 本模块只做两件事
//!
//! 1. **留痕**：Host 的 stderr 从「继承壳的 stderr」改成管道后，必须原样转到壳的 stderr
//!    （本地跑壳时那些日志是排障的唯一入口，不能让管道把它吃掉）；
//! 2. **解析**：把 `startup_failed` 那一行解成 [`StartupFailureFacts`]，交给监督线程在发布
//!    `failed` 时使用。**解析不出来就说不知道**（返回 `None` 的字段），绝不猜——错码会让渲染层
//!    按错的语义画失败屏。
//!
//! ## 与「`sequence` 归属」（DR-T3-3 ②）的关系
//!
//! 这里**没有**把 Host 的启动状态流合并进壳发布的状态流：它只读一行**终态**失败事实。
//! 壳仍然是 `DatabaseStartupState` 的唯一发布者，`sequence` 仍然只有壳一个计数器——
//! 所以 ② 那条义务不需要在这里动（详见 spec 的处置记录）。

use std::io::{BufRead, BufReader, Read};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::startup::{StartupErrorCode, StartupStage};

/// Host 报来的失败事实。两个字段各自独立地可能「认不出来」。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartupFailureFacts {
    /// 失败停在哪个应用阶段（`preparing_host_storage` 等）。
    pub failed_phase: Option<StartupStage>,
    /// Host 用 `classifyDatabaseStartupError` 分出的错码。
    pub error_code: Option<StartupErrorCode>,
}

/// Host 侧那行结构化失败留痕的标记。
const FAILURE_MARKER: &str = "startup_failed";

/// 解析一行 Host stderr，抽出结构化失败事实。
///
/// 判据（缺一不可，其余一律返回 `None`）：
///
/// - 行里出现 `startup_failed`；
/// - `phase=failed`（只有终态才算启动失败；`phase=starting` 之类的行不是）；
/// - `failedPhase` / `errorCode` 至少有一个能解到已知取值（解不出的那个留 `None`，
///   由发布方退回粗粒度值）。
///
/// **同名键取第一次出现**：错误描述里可能再出现 `errorCode=` 之类的字样，不能让它覆盖真值。
pub fn parse_startup_failure_line(line: &str) -> Option<StartupFailureFacts> {
    let marker = line.find(FAILURE_MARKER)?;
    let mut phase: Option<&str> = None;
    let mut facts = StartupFailureFacts {
        failed_phase: None,
        error_code: None,
    };
    for token in line[marker + FAILURE_MARKER.len()..].split_whitespace() {
        let Some((key, value)) = token.split_once('=') else {
            continue;
        };
        match key {
            "phase" if phase.is_none() => phase = Some(value),
            "failedPhase" if facts.failed_phase.is_none() => {
                facts.failed_phase = StartupStage::from_wire(value);
            }
            "errorCode" if facts.error_code.is_none() => {
                facts.error_code = StartupErrorCode::from_wire(value);
            }
            _ => {}
        }
    }
    if phase != Some("failed") {
        return None;
    }
    if facts.failed_phase.is_none() && facts.error_code.is_none() {
        return None;
    }
    Some(facts)
}

/// Host stderr 的观察者：留痕 + 收集失败事实 + 让监督线程能等它读完。
///
/// 「等它读完」是必需的，不是优化：Host 退出与 stderr 管道 EOF 之间没有顺序保证，
/// 监督线程若直接发布，就会在事实还没到之前把它粗粒度地盖掉。
pub struct StderrMonitor {
    facts: Mutex<StartupFailureFacts>,
    saw_facts: Mutex<bool>,
    drained: Mutex<bool>,
    drained_signal: Condvar,
}

impl StderrMonitor {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::bare())
    }

    fn bare() -> Self {
        Self {
            facts: Mutex::new(StartupFailureFacts {
                failed_phase: None,
                error_code: None,
            }),
            saw_facts: Mutex::new(false),
            drained: Mutex::new(false),
            drained_signal: Condvar::new(),
        }
    }

    /// 起读线程：逐行转到壳的 stderr，并尝试解析失败事实。
    pub fn spawn(self: &Arc<Self>, stderr: impl Read + Send + 'static) {
        let monitor = Arc::clone(self);
        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(text) => {
                        // 留痕优先：日志不能因为解析逻辑而被吃掉（本地跑壳时它是唯一入口）。
                        eprintln!("{text}");
                        monitor.absorb(&text);
                    }
                    // 非 UTF-8 或读失败：留痕已经做不到，但**必须**结束读循环并放行等待者，
                    // 否则监督线程会白等到超时（表现是失败原因退回粗粒度）。
                    Err(_) => break,
                }
            }
            monitor.mark_drained();
        });
    }

    fn absorb(&self, line: &str) {
        let Some(facts) = parse_startup_failure_line(line) else {
            return;
        };
        let mut stored = self
            .facts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // 第一次胜出：Host 只写一行，重复出现时先到的更接近真因。
        if stored.failed_phase.is_none() {
            stored.failed_phase = facts.failed_phase;
        }
        if stored.error_code.is_none() {
            stored.error_code = facts.error_code;
        }
        drop(stored);
        *self
            .saw_facts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
    }

    fn mark_drained(&self) {
        *self
            .drained
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        self.drained_signal.notify_all();
    }

    /// 等 stderr 读干（EOF）或超时。返回是否真的读干了——超时是**如实的失败**，不是成功。
    pub fn wait_drained(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut drained = self
            .drained
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while !*drained {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let (waited, result) = self
                .drained_signal
                .wait_timeout(drained, remaining)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            drained = waited;
            if result.timed_out() && !*drained {
                return false;
            }
        }
        true
    }

    /// 收集到的失败事实（`None` = Host 没报过结构化失败，调用方退回粗粒度原因）。
    pub fn facts(&self) -> Option<StartupFailureFacts> {
        if !*self
            .saw_facts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        {
            return None;
        }
        Some(
            *self
                .facts
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }
}

impl Default for StderrMonitor {
    fn default() -> Self {
        Self::bare()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真机实测的那一行（`docs/specs/tauri-renderer-host.md` §10.6 的原文形态）。
    const REAL_LINE: &str = "[mode-host-sidecar] startup_failed phase=failed failedPhase=preparing_host_storage errorCode=sql_failed error=ENOTDIR: not a directory, mkdir 'C:\\…\\mode-host-webview-Ky72BX\\data'";

    #[test]
    fn parses_the_real_startup_failure_line() {
        let facts = parse_startup_failure_line(REAL_LINE).expect("真机那行必须能解析");
        assert_eq!(
            facts.failed_phase,
            Some(StartupStage::PreparingHostStorage),
            "阶段必须精确到 Host 报的那一段（这正是 DR-T3-3 ① 的差额）"
        );
        assert_eq!(facts.error_code, Some(StartupErrorCode::SqlFailed));
    }

    #[test]
    fn ignores_lines_that_are_not_terminal_startup_failures() {
        // 不是失败行：Host 的正常日志、起 Host 的其它诊断都不能被当成失败事实。
        assert_eq!(
            parse_startup_failure_line("[mode-host-sidecar] ready startupId=x"),
            None
        );
        assert_eq!(
            parse_startup_failure_line("[sidecar-host] host 停止：host_stdout_eof"),
            None
        );
        // `startup_failed` 但 phase 不是 failed：不是终态，不能当失败事实用。
        assert_eq!(
            parse_startup_failure_line("startup_failed phase=starting errorCode=sql_failed"),
            None
        );
        // 终态但两个字段都认不出来：不猜，退回粗粒度。
        assert_eq!(
            parse_startup_failure_line("startup_failed phase=failed failedPhase=? errorCode=?"),
            None
        );
    }

    #[test]
    fn keeps_the_fields_it_can_read_when_the_other_is_unknown() {
        // 只有错码能认：阶段退回粗粒度，但错码仍然要送到渲染层（能给的比全丢好）。
        let code_only = parse_startup_failure_line(
            "startup_failed phase=failed failedPhase=? errorCode=sql_failed",
        )
        .expect("至少一个字段可解时应当返回事实");
        assert_eq!(code_only.failed_phase, None);
        assert_eq!(code_only.error_code, Some(StartupErrorCode::SqlFailed));

        let phase_only = parse_startup_failure_line(
            "startup_failed phase=failed failedPhase=starting_services errorCode=?",
        )
        .expect("至少一个字段可解时应当返回事实");
        assert_eq!(
            phase_only.failed_phase,
            Some(StartupStage::StartingServices)
        );
        assert_eq!(phase_only.error_code, None);
    }

    #[test]
    fn the_first_occurrence_of_a_key_wins() {
        // 错误描述里再出现 `errorCode=` 时不得覆盖真值（描述是自由文本，可能带着任何字样）。
        let facts = parse_startup_failure_line(
            "startup_failed phase=failed failedPhase=preparing_host_storage errorCode=permission_denied error=EACCES: errorCode=corrupt",
        )
        .expect("能解析");
        assert_eq!(
            facts.error_code,
            Some(StartupErrorCode::PermissionDenied),
            "真值在错误描述之前：先到的那个才是真值"
        );
    }

    #[test]
    fn monitor_reports_facts_and_signals_drain_after_eof() {
        let monitor = StderrMonitor::new();
        let payload = format!("{REAL_LINE}\n[mode-host-sidecar] some later log\n");
        monitor.spawn(std::io::Cursor::new(payload));
        assert!(
            monitor.wait_drained(Duration::from_secs(5)),
            "EOF 后必须放行等待者"
        );
        let facts = monitor.facts().expect("读过真行之后应当有事实");
        assert_eq!(facts.failed_phase, Some(StartupStage::PreparingHostStorage));
        assert_eq!(facts.error_code, Some(StartupErrorCode::SqlFailed));
    }

    #[test]
    fn monitor_without_failure_lines_keeps_quiet_and_still_drains() {
        let monitor = StderrMonitor::new();
        monitor.spawn(std::io::Cursor::new(
            "[mode-host-sidecar] ready startupId=x\n",
        ));
        assert!(monitor.wait_drained(Duration::from_secs(5)));
        assert_eq!(
            monitor.facts(),
            None,
            "没看到失败行就不能编一个失败事实出来"
        );
    }
}
