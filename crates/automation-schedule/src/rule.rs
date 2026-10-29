//! `scheduleRule` 的下一次触发计算与 carrier 归一化。
//!
//! 与 TS `computeScheduleRuleNextRunAt` / `buildIntervalScheduleRule` 同一语义：
//! `scheduleRule` 是调度权威，`cronExpr` 只是兼容展示；所有日历计算都在固定本地偏移下进行。

use serde::{Deserialize, Serialize};

use crate::civil::{
    civil_from_days, days_from_civil, from_local, normalized_date, to_local, weekday_from_days,
    LocalDateTime,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IntervalUnit {
    Minute,
    Hourly,
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MonthlyMode {
    Date,
    Weekday,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleRule {
    pub unit: IntervalUnit,
    pub interval: f64,
    pub hour: i64,
    pub minute: i64,
    pub anchor_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weekdays: Option<Vec<i64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub month_days: Option<Vec<i64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub months: Option<Vec<i64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monthly_mode: Option<MonthlyMode>,
}

impl ScheduleRule {
    fn step(&self) -> i64 {
        // TS: Math.max(1, Math.floor(rule.interval))
        let floored = self.interval.floor();
        if floored.is_nan() || floored < 1.0 {
            1
        } else {
            floored as i64
        }
    }
}

/// JS `Math.floor(a / b)` 的整数结果。
fn floor_div(dividend: i64, divisor: i64) -> i64 {
    (dividend as f64 / divisor as f64).floor() as i64
}

/// 纯一次性任务（非循环且至多执行一次）。
pub fn is_one_shot_automation(recurring: bool, max_runs: Option<i64>) -> bool {
    !recurring && max_runs.unwrap_or(1) <= 1
}

/// `scheduleRuleDefinition` 的规范字符串：只在展示字段变化时判等。
pub fn schedule_rule_definition(rule: &ScheduleRule) -> String {
    let unit = match rule.unit {
        IntervalUnit::Minute => "minute",
        IntervalUnit::Hourly => "hourly",
        IntervalUnit::Daily => "daily",
        IntervalUnit::Weekly => "weekly",
        IntervalUnit::Monthly => "monthly",
        IntervalUnit::Yearly => "yearly",
    };
    let monthly_mode = rule.monthly_mode.map(|mode| match mode {
        MonthlyMode::Date => "\"date\"",
        MonthlyMode::Weekday => "\"weekday\"",
    });
    format!(
        "[\"{unit}\",{},{},{},{},{},{},{}]",
        format_js_number(rule.interval),
        rule.hour,
        rule.minute,
        sorted_json(rule.weekdays.as_ref()),
        sorted_json(rule.month_days.as_ref()),
        sorted_json(rule.months.as_ref()),
        monthly_mode.unwrap_or("null"),
    )
}

fn sorted_json(values: Option<&Vec<i64>>) -> String {
    match values {
        None => "null".to_string(),
        Some(values) => {
            let mut sorted = values.clone();
            sorted.sort();
            let parts: Vec<String> = sorted.iter().map(|value| value.to_string()).collect();
            format!("[{}]", parts.join(","))
        }
    }
}

pub fn format_js_number(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1e21 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// 「几分钟后」→ 以服务端真实时钟为锚点的一次性规则（返回兼容 cron 展示 + 权威规则）。
pub fn build_relative_delay_schedule(
    delay_minutes: f64,
    from: i64,
    offset_secs: i32,
) -> (String, ScheduleRule) {
    let target = from + (delay_minutes * 60_000.0) as i64;
    let local = to_local(target, offset_secs);
    let cron_expr = format!(
        "{} {} {} {} *",
        local.minute,
        local.hour,
        local.day,
        local.month
    );
    let rule = ScheduleRule {
        unit: IntervalUnit::Minute,
        interval: delay_minutes,
        hour: local.hour as i64,
        minute: local.minute as i64,
        anchor_at: from,
        weekdays: None,
        month_days: None,
        months: None,
        monthly_mode: None,
    };
    (cron_expr, rule)
}

/// 「每 N 分钟」的兼容 cron 展示 → 带创建时刻锚点的产品调度规则。
pub fn infer_minute_interval_schedule_rule(
    cron_expr: &str,
    anchor_at: i64,
    offset_secs: i32,
) -> Option<ScheduleRule> {
    let parts: Vec<&str> = cron_expr.split_whitespace().collect();
    if parts.len() != 5 {
        return None;
    }
    let rest = parts[0].strip_prefix("*/")?;
    if rest.is_empty() || !rest.chars().all(|ch| ch.is_ascii_digit()) || rest.starts_with('0') {
        return None;
    }
    if parts[1..].iter().any(|part| *part != "*") {
        return None;
    }
    let interval: f64 = rest.parse().ok()?;
    let local = to_local(anchor_at, offset_secs);
    Some(ScheduleRule {
        unit: IntervalUnit::Minute,
        interval,
        hour: local.hour as i64,
        minute: local.minute as i64,
        anchor_at,
        weekdays: None,
        month_days: None,
        months: None,
        monthly_mode: None,
    })
}

#[derive(Debug, Default)]
struct CronFields {
    minute: Option<i64>,
    hour: Option<i64>,
    month_days: Option<Vec<i64>>,
    months: Option<Vec<i64>>,
    weekdays: Option<Vec<i64>>,
}

fn parse_cron_fields(cron_expr: &str) -> CronFields {
    let parts: Vec<&str> = cron_expr.split_whitespace().collect();
    if parts.len() < 5 {
        return CronFields::default();
    }
    let parse_single = |token: &str| -> Option<i64> {
        if token == "*" || token == "?" {
            return None;
        }
        parse_js_integer(token)
    };
    let parse_list = |token: &str| -> Option<Vec<i64>> {
        if token == "*" || token == "?" {
            return None;
        }
        let values: Vec<i64> = token
            .split(',')
            .filter_map(parse_js_integer)
            .collect();
        if values.is_empty() {
            None
        } else {
            Some(values)
        }
    };
    CronFields {
        minute: parse_single(parts[0]),
        hour: parse_single(parts[1]),
        month_days: parse_list(parts[2]),
        months: parse_list(parts[3]),
        weekdays: parse_list(parts[4]),
    }
}

/// JS `Number(token)` 且 `Number.isInteger`：空串按 0，非数字与小数返回 None。
fn parse_js_integer(token: &str) -> Option<i64> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return Some(0);
    }
    match trimmed.parse::<i64>() {
        Ok(value) => Some(value),
        Err(_) => {
            let value = trimmed.parse::<f64>().ok()?;
            if value.is_finite() && value.fract() == 0.0 && value.abs() < 9e15 {
                Some(value as i64)
            } else {
                None
            }
        }
    }
}

/// 会话侧 carrier（intervalUnit + interval）归一化为权威 `scheduleRule`。
pub fn build_interval_schedule_rule(
    unit: IntervalUnit,
    interval: f64,
    cron_expr: &str,
    anchor_at: i64,
    offset_secs: i32,
) -> ScheduleRule {
    let fields = parse_cron_fields(cron_expr);
    let anchor = to_local(anchor_at, offset_secs);
    let minute = fields.minute.unwrap_or(anchor.minute as i64);
    let hour = fields.hour.unwrap_or(anchor.hour as i64);
    let month_days = fields
        .month_days
        .unwrap_or_else(|| vec![anchor.day as i64]);
    let months = fields
        .months
        .unwrap_or_else(|| vec![anchor.month as i64]);
    let weekdays = fields.weekdays.unwrap_or_else(|| vec![1]);
    let base = ScheduleRule {
        unit,
        interval,
        hour,
        minute,
        anchor_at,
        weekdays: None,
        month_days: None,
        months: None,
        monthly_mode: None,
    };
    match unit {
        IntervalUnit::Minute => ScheduleRule {
            hour: anchor.hour as i64,
            minute: anchor.minute as i64,
            ..base
        },
        IntervalUnit::Hourly => ScheduleRule {
            hour: 0,
            ..base
        },
        IntervalUnit::Daily => base,
        IntervalUnit::Weekly => ScheduleRule {
            weekdays: Some(weekdays),
            ..base
        },
        IntervalUnit::Monthly => ScheduleRule {
            month_days: Some(month_days),
            monthly_mode: Some(MonthlyMode::Date),
            ..base
        },
        IntervalUnit::Yearly => ScheduleRule {
            months: Some(months),
            month_days: Some(month_days),
            ..base
        },
    }
}

/// 自定义重复规则的下一次运行（严格晚于 `from`）。
pub fn compute_schedule_rule_next_run_at(
    rule: &ScheduleRule,
    from: i64,
    offset_secs: i32,
) -> Option<i64> {
    let interval = rule.step();
    let anchor = to_local(rule.anchor_at, offset_secs);
    match rule.unit {
        IntervalUnit::Minute => {
            let step = interval * 60_000;
            let mut steps = floor_div(from - rule.anchor_at, step) + 1;
            if steps < 1 {
                steps = 1;
            }
            Some(rule.anchor_at + steps * step)
        }
        IntervalUnit::Hourly => {
            let base = from_local(
                anchor.year,
                anchor.month as i64,
                anchor.day as i64,
                anchor.hour as i64,
                rule.minute,
                offset_secs,
            );
            let step = interval * 3_600_000;
            let mut steps = floor_div(from - base, step) + 1;
            if steps < 0 {
                steps = 0;
            }
            Some(base + steps * step)
        }
        IntervalUnit::Daily => {
            let anchor_days = days_from_civil(anchor.year, anchor.month, anchor.day);
            for index in 0..36_600i64 {
                let (year, month, day) = civil_from_days(anchor_days + index * interval);
                let candidate =
                    from_local(year, month as i64, day as i64, rule.hour, rule.minute, offset_secs);
                if candidate > from {
                    return Some(candidate);
                }
            }
            None
        }
        IntervalUnit::Weekly => {
            let anchor_days = days_from_civil(anchor.year, anchor.month, anchor.day);
            let anchor_week = anchor_days - ((anchor.weekday + 6) % 7) as i64;
            let mut weekdays: Vec<i64> = match &rule.weekdays {
                Some(values) if !values.is_empty() => values.clone(),
                _ => vec![1],
            };
            weekdays.sort();
            let mut week = 0i64;
            while week < 5_220 {
                for weekday in &weekdays {
                    let day_offset = (weekday + 6).rem_euclid(7);
                    let (year, month, day) =
                        civil_from_days(anchor_week + week * 7 + day_offset);
                    let candidate = from_local(
                        year,
                        month as i64,
                        day as i64,
                        rule.hour,
                        rule.minute,
                        offset_secs,
                    );
                    if candidate > from {
                        return Some(candidate);
                    }
                }
                week += interval;
            }
            None
        }
        IntervalUnit::Monthly => {
            let mut offset = 0i64;
            while offset <= 1_200 {
                let (year, month) = month_at(&anchor, offset);
                let candidates = if rule.monthly_mode == Some(MonthlyMode::Weekday) {
                    let weekday = rule
                        .weekdays
                        .as_ref()
                        .and_then(|values| values.first())
                        .copied()
                        .unwrap_or(1);
                    vec![first_weekday_of_month(year, month, weekday)]
                } else {
                    let mut days: Vec<i64> = match &rule.month_days {
                        Some(values) if !values.is_empty() => values.clone(),
                        _ => vec![1],
                    };
                    days.sort();
                    days.iter()
                        .map(|day| normalized_date(year, month as i64, *day))
                        .filter(|(_, candidate_month, _)| *candidate_month == month)
                        .collect()
                };
                for (year, month, day) in candidates {
                    let candidate =
                        from_local(year, month as i64, day as i64, rule.hour, rule.minute, offset_secs);
                    if candidate > from {
                        return Some(candidate);
                    }
                }
                offset += interval;
            }
            None
        }
        IntervalUnit::Yearly => {
            let target_month = match rule.months.as_ref().and_then(|values| values.first()) {
                Some(month) => ((month - 1).rem_euclid(12)) as u32,
                None => anchor.month - 1,
            };
            let target_day = rule
                .month_days
                .as_ref()
                .and_then(|values| values.first())
                .copied()
                .unwrap_or(anchor.day as i64);
            let mut offset = 0i64;
            while offset < 400 {
                let (year, month, day) =
                    normalized_date(anchor.year + offset, target_month as i64 + 1, target_day);
                if month - 1 != target_month {
                    offset += interval;
                    continue;
                }
                let candidate =
                    from_local(year, month as i64, day as i64, rule.hour, rule.minute, offset_secs);
                if candidate > from {
                    return Some(candidate);
                }
                offset += interval;
            }
            None
        }
    }
}

fn month_at(anchor: &LocalDateTime, offset: i64) -> (i64, u32) {
    let total = anchor.year * 12 + (anchor.month - 1) as i64 + offset;
    (
        total.div_euclid(12),
        (total.rem_euclid(12) as u32) + 1,
    )
}

fn first_weekday_of_month(year: i64, month: u32, weekday: i64) -> (i64, u32, u32) {
    let first_days = days_from_civil(year, month, 1);
    let first_weekday = weekday_from_days(first_days) as i64;
    let delta = (weekday - first_weekday + 7).rem_euclid(7);
    civil_from_days(first_days + delta)
}
