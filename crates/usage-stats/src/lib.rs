//! App Usage / Coding Plan 时间范围与坐标轴解析。
//!
//! 真源：`packages/services/src/usage-stats/providers/bigmodelUsageMonitorRange.ts`。
//! 不读系统时钟或 IANA 时区；调用方注入 `now` 与固定本地偏移秒数。

mod mcp_quota;
mod monitor;
mod monitor_credit;
mod quota;

use serde_json::{json, Value};

pub use mcp_quota::{
    build_mcp_quota_aggregate_limit, matches_mcp_quota_scope, parse_mcp_usage_envelope,
    McpQuotaRequestScope, McpUsageSnapshot, McpUsageTotal, UsageMcpQuotaScope,
    MCP_USAGE_QUOTA_LIMIT_TYPE,
};
pub use monitor::build_usage_stats_snapshot_from_monitor;
pub use monitor_credit::build_coding_plan_usage_snapshot_from_monitor;
pub use quota::{
    normalize_limits, pick_current_subscription_from_list, pick_primary_limit,
    BigModelSubscriptionListItem, CurrentSubscriptionSummary, UsageQuotaLimit,
    UsageQuotaUsageDetail,
};

const MONITOR_MAX_RANGE_DAYS: i64 = 30;
const MONITOR_DEFAULT_RANGE_DAYS: i64 = 30;
const MS_PER_DAY: i64 = 86_400_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageStatsRange {
    All,
    SevenDays,
    ThirtyDays,
}

impl UsageStatsRange {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "all" => Some(Self::All),
            "7d" => Some(Self::SevenDays),
            "30d" => Some(Self::ThirtyDays),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodingPlanUsageRange {
    Today,
    SevenDays,
    ThirtyDays,
    Custom,
}

impl CodingPlanUsageRange {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "today" => Some(Self::Today),
            "7d" => Some(Self::SevenDays),
            "30d" => Some(Self::ThirtyDays),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodingPlanUsageGranularity {
    Hour,
    Day,
}

impl CodingPlanUsageGranularity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hour => "hour",
            Self::Day => "day",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageTimeRange {
    pub start_time: String,
    pub end_time: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodingPlanUsageTimeRange {
    pub start_time: String,
    pub end_time: String,
    pub granularity: CodingPlanUsageGranularity,
    pub x_time: Vec<String>,
    pub start_date_key: String,
    pub end_date_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodingPlanRangeRequest {
    pub range: CodingPlanUsageRange,
    pub custom_start_date: Option<String>,
    pub custom_end_date: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageTimeRangeError {
    CustomRangeInvalidOrder,
    CustomRangeTooLong,
    CustomStartDateInvalid,
    CustomEndDateInvalid,
}

impl UsageTimeRangeError {
    pub fn message(self) -> &'static str {
        match self {
            Self::CustomRangeInvalidOrder => "coding_plan_usage_custom_range_invalid_order",
            Self::CustomRangeTooLong => "coding_plan_usage_custom_range_too_long",
            Self::CustomStartDateInvalid => "coding_plan_usage_custom_start_date_invalid",
            Self::CustomEndDateInvalid => "coding_plan_usage_custom_end_date_invalid",
        }
    }
}

impl std::fmt::Display for UsageTimeRangeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for UsageTimeRangeError {}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { y + 1 } else { y }, month, day)
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let month = month as i64;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn local_day(now_ms: i64, offset_secs: i32) -> i64 {
    (now_ms + offset_secs as i64 * 1000).div_euclid(MS_PER_DAY)
}

fn date_key_from_days(days: i64) -> String {
    let (year, month, day) = civil_from_days(days);
    format!("{year}-{month:02}-{day:02}")
}

fn local_date_key(now_ms: i64, offset_secs: i32) -> String {
    date_key_from_days(local_day(now_ms, offset_secs))
}

/// `Date.UTC(year, month - 1, day)` 的日数；月/日溢出按 JS 构造器语义滚动。
fn date_key_to_days(date_key: &str) -> i64 {
    let mut parts = date_key.split('-');
    let year = parts.next().unwrap_or("1970").parse::<i64>().unwrap_or(0);
    let month = parts.next().unwrap_or("01").parse::<i64>().unwrap_or(1);
    let day = parts.next().unwrap_or("01").parse::<i64>().unwrap_or(1);
    let total_months = year * 12 + (month - 1);
    let normalized_year = total_months.div_euclid(12);
    let normalized_month = (total_months.rem_euclid(12) + 1) as u32;
    days_from_civil(normalized_year, normalized_month, 1) + day - 1
}

fn add_days_to_date_key(date_key: &str, days: i64) -> String {
    date_key_from_days(date_key_to_days(date_key) + days)
}

fn format_local_date_time(day: i64, second_of_day: i64) -> String {
    let (year, month, day) = civil_from_days(day);
    format!(
        "{year}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        second_of_day / 3600,
        (second_of_day % 3600) / 60,
        second_of_day % 60
    )
}

fn build_hourly_axis() -> Vec<String> {
    (0..24).map(|hour| format!("{hour:02}:00:00")).collect()
}

fn build_daily_axis(start_date_key: &str, days: i64) -> Vec<String> {
    (0..days)
        .map(|index| add_days_to_date_key(start_date_key, index))
        .collect()
}

fn normalize_custom_date_key(
    value: Option<&str>,
    label: &str,
) -> Result<String, UsageTimeRangeError> {
    let trimmed = value.unwrap_or("").trim();
    let bytes = trimmed.as_bytes();
    let valid = bytes.len() == 10
        && bytes[0..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit);
    if !valid {
        return Err(if label == "start" {
            UsageTimeRangeError::CustomStartDateInvalid
        } else {
            UsageTimeRangeError::CustomEndDateInvalid
        });
    }
    Ok(trimmed.to_string())
}

pub fn resolve_usage_time_range(
    range: UsageStatsRange,
    now_ms: i64,
    offset_secs: i32,
) -> UsageTimeRange {
    let days = match range {
        UsageStatsRange::SevenDays => 7,
        UsageStatsRange::All | UsageStatsRange::ThirtyDays => MONITOR_DEFAULT_RANGE_DAYS,
    };
    let today = local_day(now_ms, offset_secs);
    let end_second = 23 * 3600 + 59 * 60 + 59;
    let start_day = today - (days.min(MONITOR_MAX_RANGE_DAYS) - 1);
    UsageTimeRange {
        start_time: format_local_date_time(start_day, 0),
        end_time: format_local_date_time(today, end_second),
    }
}

pub fn resolve_coding_plan_usage_time_range(
    request: &CodingPlanRangeRequest,
    now_ms: i64,
    offset_secs: i32,
) -> Result<CodingPlanUsageTimeRange, UsageTimeRangeError> {
    let end_date_key = local_date_key(now_ms, offset_secs);
    match request.range {
        CodingPlanUsageRange::Today => Ok(CodingPlanUsageTimeRange {
            start_time: format!("{end_date_key} 00:00:00"),
            end_time: format!("{end_date_key} 23:59:59"),
            granularity: CodingPlanUsageGranularity::Hour,
            x_time: build_hourly_axis(),
            start_date_key: end_date_key.clone(),
            end_date_key,
        }),
        CodingPlanUsageRange::SevenDays | CodingPlanUsageRange::ThirtyDays => {
            let window = if request.range == CodingPlanUsageRange::SevenDays {
                7
            } else {
                30
            };
            let start_date_key = add_days_to_date_key(&end_date_key, -(window - 1));
            Ok(CodingPlanUsageTimeRange {
                start_time: format!("{start_date_key} 00:00:00"),
                end_time: format!("{end_date_key} 23:59:59"),
                granularity: CodingPlanUsageGranularity::Day,
                x_time: build_daily_axis(&start_date_key, window),
                start_date_key,
                end_date_key,
            })
        }
        CodingPlanUsageRange::Custom => {
            let start_date_key =
                normalize_custom_date_key(request.custom_start_date.as_deref(), "start")?;
            let custom_end_date_key =
                normalize_custom_date_key(request.custom_end_date.as_deref(), "end")?;
            let days =
                date_key_to_days(&custom_end_date_key) - date_key_to_days(&start_date_key) + 1;
            if days <= 0 {
                return Err(UsageTimeRangeError::CustomRangeInvalidOrder);
            }
            if days > MONITOR_MAX_RANGE_DAYS {
                return Err(UsageTimeRangeError::CustomRangeTooLong);
            }
            Ok(CodingPlanUsageTimeRange {
                start_time: format!("{start_date_key} 00:00:00"),
                end_time: format!("{custom_end_date_key} 23:59:59"),
                granularity: CodingPlanUsageGranularity::Day,
                x_time: build_daily_axis(&start_date_key, days),
                start_date_key: start_date_key.clone(),
                end_date_key: custom_end_date_key,
            })
        }
    }
}

impl UsageTimeRange {
    pub fn to_json(&self) -> Value {
        json!({ "startTime": self.start_time, "endTime": self.end_time })
    }
}

impl CodingPlanUsageTimeRange {
    pub fn to_json(&self) -> Value {
        json!({
            "startTime": self.start_time,
            "endTime": self.end_time,
            "granularity": self.granularity.as_str(),
            "xTime": self.x_time,
            "startDateKey": self.start_date_key,
            "endDateKey": self.end_date_key,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_range_uses_whole_local_days() {
        let now = days_from_civil(2026, 1, 2) * MS_PER_DAY + 12 * 3_600_000;
        let range = resolve_usage_time_range(UsageStatsRange::SevenDays, now, 0);
        assert_eq!(range.end_time, "2026-01-02 23:59:59");
        assert_eq!(range.start_time, "2025-12-27 00:00:00");
    }

    #[test]
    fn custom_range_normalizes_month_overflow() {
        let request = CodingPlanRangeRequest {
            range: CodingPlanUsageRange::Custom,
            custom_start_date: Some("2026-13-01".to_string()),
            custom_end_date: Some("2027-01-02".to_string()),
        };
        let range = resolve_coding_plan_usage_time_range(&request, 0, 0).expect("range");
        assert_eq!(range.start_date_key, "2026-13-01");
        assert_eq!(range.end_date_key, "2027-01-02");
        assert_eq!(range.x_time, vec!["2027-01-01", "2027-01-02"]);
    }
}
