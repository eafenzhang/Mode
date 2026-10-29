//! `automation-schedule`：定时任务的调度纯逻辑（Rust 实现）。
//!
//! 行为真源：`docs/specs/automation-schedule.md`；对拍对象是本仓库 TS
//! `packages/services/src/session/automationCron.ts`、
//! `automationCronValidation.ts` 与 `automationIntervalCarrier.ts`。
//!
//! 本 crate 不读时钟、不碰数据库：所有入口都由调用方传入 `from`（毫秒时间戳）与
//! **固定的本地时区偏移秒数**。偏移固定意味着「夏令时跃迁被视作同一偏移」，
//! 见 spec「已知差异」。

mod civil;
mod cron;
mod rule;

pub use cron::{next_run, previous_runs, CronSpec};
pub use rule::{
    build_interval_schedule_rule, build_relative_delay_schedule, compute_schedule_rule_next_run_at,
    format_js_number, infer_minute_interval_schedule_rule, is_one_shot_automation,
    schedule_rule_definition, IntervalUnit, MonthlyMode, ScheduleRule,
};

use civil::to_local;
use serde_json::{Map, Value};

/// 一次性固定月日 cron 刚错过的容忍窗口：超过一分钟的目标不能安全补执行。
const ONE_SHOT_MISSED_RUN_GRACE_MS: i64 = 60 * 1_000;
/// 「目标刚过去」的识别窗口，覆盖模型对当前时刻的陈旧估计。
const ONE_SHOT_STALE_TARGET_WINDOW_MS: i64 = 30 * 60 * 1_000;

/// 自定义重复 carrier 的受控上限。
const MAX_SESSION_AUTOMATION_INTERVAL: i64 = 200;

/// 一次性任务的固定日历 cron 目标时间已过去、且不能安全补执行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleOneShotScheduleError {
    /// 用户以为的目标时刻（毫秒）。
    pub target_at: i64,
}

impl std::fmt::Display for StaleOneShotScheduleError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "一次性定时任务的目标时间（{}）已过去；相对时间请使用 delayMinutes，绝对时间请确认未来时刻后重试",
            format_local_datetime(self.target_at, 0)
        )
    }
}

impl std::error::Error for StaleOneShotScheduleError {}

/// 会话侧自定义重复 carrier 配对、范围或互斥校验失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidAutomationIntervalCarrierError(pub String);

impl std::fmt::Display for InvalidAutomationIntervalCarrierError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "非法的自定义重复入参：{}", self.0)
    }
}

impl std::error::Error for InvalidAutomationIntervalCarrierError {}

fn format_local_datetime(ms: i64, offset_secs: i32) -> String {
    let local = to_local(ms, offset_secs);
    format!(
        "{}/{}/{} {:02}:{:02}:{:02}",
        local.year, local.month, local.day, local.hour, local.minute, local.second
    )
}

/// cron 表达式是否合法（与 TS `isValidCronExpr` 同口径）。
pub fn is_valid_cron_expr(cron_expr: &str) -> bool {
    CronSpec::parse(cron_expr).is_ok()
}

/// cron 的下一次触发；无未来触发返回 `None`。
pub fn compute_next_run_at(cron_expr: &str, from: i64, offset_secs: i32) -> Option<i64> {
    let spec = CronSpec::parse(cron_expr).ok()?;
    next_run(&spec, from, offset_secs)
}

/// 严格早于 `from` 的最近 `count` 次触发（由近到远）；非法表达式返回空表。
pub fn compute_previous_runs(
    cron_expr: &str,
    from: i64,
    count: usize,
    offset_secs: i32,
) -> Vec<i64> {
    match CronSpec::parse(cron_expr) {
        Ok(spec) => previous_runs(&spec, from, count, offset_secs),
        Err(_) => Vec::new(),
    }
}

/// 有 `scheduleRule` 时以规则为权威，否则回退 cron。
pub fn compute_automation_next_run_at(
    cron_expr: &str,
    schedule_rule: Option<&ScheduleRule>,
    from: i64,
    offset_secs: i32,
) -> Option<i64> {
    match schedule_rule {
        Some(rule) => compute_schedule_rule_next_run_at(rule, from, offset_secs),
        None => compute_next_run_at(cron_expr, from, offset_secs),
    }
}

fn is_fixed_calendar_cron(cron_expr: &str) -> bool {
    let parts: Vec<&str> = cron_expr.split_whitespace().collect();
    parts.len() == 5
        && parts[..4]
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|ch| ch.is_ascii_digit()))
        && parts[4] == "*"
}

/// 新建任务的首次触发时间。
///
/// 一次性固定月日 cron 刚错过目标分钟时会滚到下一年：一分钟内的误差立即补执行，
/// 已陈旧的目标拒绝（由调用方提示改用 `delayMinutes`）。
pub fn compute_initial_automation_next_run_at(
    cron_expr: &str,
    recurring: bool,
    schedule_rule: Option<&ScheduleRule>,
    from: i64,
    offset_secs: i32,
) -> Result<Option<i64>, StaleOneShotScheduleError> {
    let next_run_at = compute_automation_next_run_at(cron_expr, schedule_rule, from, offset_secs);
    if recurring || schedule_rule.is_some() || !is_fixed_calendar_cron(cron_expr) {
        return Ok(next_run_at);
    }

    let target_age = compute_previous_runs(cron_expr, from, 1, offset_secs)
        .first()
        .copied()
        .filter(|previous| from >= *previous && from - *previous <= ONE_SHOT_STALE_TARGET_WINDOW_MS)
        .map(|previous| from - previous);
    let rolled_to_next_calendar_occurrence = match next_run_at {
        None => true,
        Some(next) => next - from > ONE_SHOT_STALE_TARGET_WINDOW_MS,
    };
    if let Some(age) = target_age {
        if rolled_to_next_calendar_occurrence {
            if age < ONE_SHOT_MISSED_RUN_GRACE_MS {
                return Ok(Some(from));
            }
            return Err(StaleOneShotScheduleError {
                target_at: from - age,
            });
        }
    }
    Ok(next_run_at)
}

/// carrier 入参：用原始 JSON 判断「字段是否存在」，因为 `scheduleRule: null` 与缺省语义不同。
pub type CarrierInput = Map<String, Value>;

fn js_number(value: Option<&Value>) -> Option<f64> {
    match value {
        Some(Value::Number(number)) => number.as_f64(),
        _ => None,
    }
}

/// 校验会话 Cron 工具与 UI 共用的 interval carrier。
pub fn assert_valid_automation_interval_carrier(
    input: &CarrierInput,
) -> Result<(), InvalidAutomationIntervalCarrierError> {
    let interval_unit = input.get("intervalUnit").is_some();
    let interval = input.get("interval");
    if interval_unit != interval.is_some() {
        return Err(InvalidAutomationIntervalCarrierError(
            "intervalUnit 与 interval 必须同时提交或同时省略".to_string(),
        ));
    }
    if let Some(value) = interval {
        let number = js_number(Some(value));
        let valid = match number {
            Some(number) => {
                number.fract() == 0.0
                    && number >= 1.0
                    && number <= MAX_SESSION_AUTOMATION_INTERVAL as f64
            }
            None => false,
        };
        if !valid {
            return Err(InvalidAutomationIntervalCarrierError(
                "interval 必须是 1-200 的整数".to_string(),
            ));
        }
    }
    if interval_unit && input.contains_key("relativeDelayMinutes") {
        return Err(InvalidAutomationIntervalCarrierError(
            "intervalUnit 是周期 carrier，不能与一次性 relativeDelayMinutes 同时提交".to_string(),
        ));
    }
    if interval_unit && input.contains_key("scheduleRule") {
        return Err(InvalidAutomationIntervalCarrierError(
            "intervalUnit carrier 不能与直传 scheduleRule 同时提交".to_string(),
        ));
    }
    // carrier 的真实语义是无限循环：放行一次性会留下 scheduleRule 与生命周期模式矛盾的数据。
    if interval_unit && matches!(input.get("recurring"), Some(Value::Bool(false))) {
        return Err(InvalidAutomationIntervalCarrierError(
            "intervalUnit carrier 必须使用 recurring=true".to_string(),
        ));
    }
    if interval_unit && matches!(input.get("maxRuns"), Some(Value::Number(_))) {
        return Err(InvalidAutomationIntervalCarrierError(
            "intervalUnit carrier 不能与有限 maxRuns 同时提交".to_string(),
        ));
    }
    Ok(())
}

/// carrier 更新必须原子切换无限循环，避免 scheduleRule 与生命周期模式脱节。
pub fn force_interval_carrier_recurring(params: &CarrierInput) -> CarrierInput {
    let mut next = params.clone();
    next.insert("recurring".to_string(), Value::Bool(true));
    next.insert("maxRuns".to_string(), Value::Null);
    next
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_shot_definition() {
        assert!(is_one_shot_automation(false, None));
        assert!(is_one_shot_automation(false, Some(1)));
        assert!(!is_one_shot_automation(false, Some(3)));
        assert!(!is_one_shot_automation(true, None));
        assert!(!is_one_shot_automation(true, Some(1)));
    }

    #[test]
    fn cron_parses_standard_forms() {
        for expr in [
            "* * * * *",
            "*/5 * * * *",
            "0 9 * * 1-5",
            "0 9 * * MON-FRI",
            "0 0 L * *",
            "0 0 15W * *",
            "0 0 * * 5L",
            "0 0 * * 1#5",
            "0 0 0 1 1 * 2028,2030",
            "@daily",
            "30 * * * * *",
        ] {
            assert!(
                is_valid_cron_expr(expr),
                "{expr} 应合法：{:?}",
                CronSpec::parse(expr).err()
            );
        }
        for expr in ["", "0 9 * *", "61 9 * * *", "50-10 * * * *", "5/15 * * * *", "@weekdays"] {
            assert!(!is_valid_cron_expr(expr), "{expr} 应非法");
        }
    }
}
