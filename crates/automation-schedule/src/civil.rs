//! 公历日期与本地时间换算。
//!
//! TS 侧全部用 `Date` 的**本地时区**做日历计算。Rust 侧不绑定时区：调用方传入
//! 「该时刻所处的本地偏移秒数」，本模块只做 `UTC 毫秒 + 偏移 → 墙钟字段` 与反向换算。
//! 偏移在整段计算内视为常数（见 spec「已知差异」：夏令时跃迁不在本轮口径内）。

/// 本地墙钟字段。`month` 为 1-12，`weekday` 为 0=周日（与 JS `getDay()` 一致）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalDateTime {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub weekday: u32,
}

const MS_PER_DAY: i64 = 86_400_000;

/// 公历 → 1970-01-01 起的天数（Howard Hinnant `days_from_civil`）。
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let month = month as i64;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 1970-01-01 起的天数 → 公历 (year, month, day)。
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 0=周日 … 6=周六（1970-01-01 是周四）。
pub fn weekday_from_days(days: i64) -> u32 {
    (days + 4).rem_euclid(7) as u32
}

pub fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

pub fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// UTC 毫秒 + 本地偏移 → 墙钟字段。毫秒部分丢弃。
pub fn to_local(utc_ms: i64, offset_secs: i32) -> LocalDateTime {
    let local_ms = utc_ms + offset_secs as i64 * 1000;
    let days = local_ms.div_euclid(MS_PER_DAY);
    let ms_of_day = local_ms.rem_euclid(MS_PER_DAY);
    let (year, month, day) = civil_from_days(days);
    let sec_of_day = ms_of_day / 1000;
    LocalDateTime {
        year,
        month,
        day,
        hour: (sec_of_day / 3600) as u32,
        minute: ((sec_of_day % 3600) / 60) as u32,
        second: (sec_of_day % 60) as u32,
        weekday: weekday_from_days(days),
    }
}

/// 墙钟字段（允许 `hour` / `minute` 溢出，按 JS `Date` 构造器语义归一）→ UTC 毫秒。
pub fn from_local(year: i64, month: i64, day: i64, hour: i64, minute: i64, offset_secs: i32) -> i64 {
    // JS `new Date(y, m, d, h, mi)` 把月/日/时分都当作可溢出的增量。
    let month0 = month - 1;
    let total_months = year * 12 + month0;
    let norm_year = total_months.div_euclid(12);
    let norm_month = total_months.rem_euclid(12) as u32 + 1;
    let base_days = days_from_civil(norm_year, norm_month, 1) + (day - 1);
    let total_minutes = hour * 60 + minute;
    let extra_days = total_minutes.div_euclid(1440);
    let minute_of_day = total_minutes.rem_euclid(1440);
    let days = base_days + extra_days;
    days * MS_PER_DAY + minute_of_day * 60_000 - offset_secs as i64 * 1000
}

/// 归一化后的日历日期（月/日溢出按 JS 构造器语义滚动）。
pub fn normalized_date(year: i64, month: i64, day: i64) -> (i64, u32, u32) {
    let total_months = year * 12 + (month - 1);
    let norm_year = total_months.div_euclid(12);
    let norm_month = total_months.rem_euclid(12) as u32 + 1;
    let days = days_from_civil(norm_year, norm_month, 1) + (day - 1);
    civil_from_days(days)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_round_trip() {
        for days in [-100_000i64, -1, 0, 1, 20_000, 100_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(weekday_from_days(0), 4);
    }

    #[test]
    fn local_conversion_respects_offset() {
        let utc = days_from_civil(2026, 2, 28) * MS_PER_DAY + 12 * 3_600_000; // 2026-02-28T12:00:00Z
        let utc_local = to_local(utc, 0);
        assert_eq!((utc_local.year, utc_local.month, utc_local.day), (2026, 2, 28));
        assert_eq!(utc_local.hour, 12);
        let shanghai = to_local(utc, 8 * 3600);
        assert_eq!(shanghai.hour, 20);
        assert_eq!(from_local(2026, 2, 28, 20, 0, 8 * 3600), utc);
    }
}
