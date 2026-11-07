//! 最小 `httpdate` 实现：hyper server 的 `Date` 头只需要 IMF-fixdate 格式化。
//!
//! 上游 `httpdate` crate 在离线环境不可获取；本实现只覆盖
//! `HttpDate::from(SystemTime)` + `Display`（29 字节 IMF-fixdate）与解析，
//! 供 `[patch.crates-io]` 在无网络构建时使用。

use std::fmt;
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpDate(pub SystemTime);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseError;

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid HTTP date")
    }
}

impl std::error::Error for ParseError {}

impl From<SystemTime> for HttpDate {
    fn from(time: SystemTime) -> Self {
        Self(time)
    }
}

impl From<HttpDate> for SystemTime {
    fn from(date: HttpDate) -> Self {
        date.0
    }
}

impl fmt::Display for HttpDate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let duration = self
            .0
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO);
        let seconds = duration.as_secs() as i64;
        let days = seconds.div_euclid(86_400);
        let seconds_of_day = seconds.rem_euclid(86_400);
        let weekday = (days + 4).rem_euclid(7) as usize;
        let (year, month, day) = civil_from_days(days);
        let hour = seconds_of_day / 3_600;
        let minute = (seconds_of_day / 60) % 60;
        let second = seconds_of_day % 60;
        write!(
            formatter,
            "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
            WEEKDAYS[weekday],
            day,
            MONTHS[(month - 1) as usize],
            year,
            hour,
            minute,
            second
        )
    }
}

impl FromStr for HttpDate {
    type Err = ParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        parse_http_date(value).map(HttpDate)
    }
}

/// 解析 IMF-fixdate：`Sun, 06 Nov 1994 08:49:37 GMT`。
pub fn parse_http_date(value: &str) -> Result<SystemTime, ParseError> {
    let bytes = value.as_bytes();
    if bytes.len() != 29
        || bytes[3] != b','
        || bytes[4] != b' '
        || bytes[7] != b' '
        || bytes[11] != b' '
        || bytes[16] != b' '
        || bytes[19] != b':'
        || bytes[22] != b':'
        || bytes[25] != b' '
        || &value[26..29] != "GMT"
    {
        return Err(ParseError);
    }
    let weekday = &value[0..3];
    if !WEEKDAYS.contains(&weekday) {
        return Err(ParseError);
    }
    let day = number(&value[5..7])?;
    let month = MONTHS
        .iter()
        .position(|month| *month == &value[8..11])
        .ok_or(ParseError)? as u32
        + 1;
    let year = number(&value[12..16])?;
    let hour = number(&value[17..19])?;
    let minute = number(&value[20..22])?;
    let second = number(&value[23..25])?;
    if day == 0 || day > 31 || hour > 23 || minute > 59 || second > 59 {
        return Err(ParseError);
    }
    let days = days_from_civil(year as i64, month, day);
    let seconds = days
        .checked_mul(86_400)
        .and_then(|value| value.checked_add((hour * 3_600 + minute * 60 + second) as i64))
        .ok_or(ParseError)?;
    if seconds < 0 {
        return Err(ParseError);
    }
    Ok(UNIX_EPOCH + Duration::from_secs(seconds as u64))
}

pub fn fmt_http_date(time: SystemTime) -> String {
    HttpDate::from(time).to_string()
}

fn number(value: &str) -> Result<u32, ParseError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ParseError);
    }
    value.parse().map_err(|_| ParseError)
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
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

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_and_parses_imf_fixdate() {
        let time = UNIX_EPOCH + Duration::from_secs(784_111_777); // 1994-11-06T08:49:37Z
        assert_eq!(fmt_http_date(time), "Sun, 06 Nov 1994 08:49:37 GMT");
        assert_eq!(parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT").unwrap(), time);
    }

    #[test]
    fn rejects_malformed_dates() {
        assert!(parse_http_date("not a date").is_err());
        assert!(parse_http_date("Sun, 06 Xxx 1994 08:49:37 GMT").is_err());
        assert!(parse_http_date("Sun, 06 Nov 1994 24:00:00 GMT").is_err());
    }
}
