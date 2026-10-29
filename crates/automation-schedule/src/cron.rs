//! 五段/六段/七段 cron 解析与下次/上次触发计算。
//!
//! 语义对齐本仓库 TS 侧依赖的 croner 10（行为真源见 `docs/specs/automation-schedule.md`）：
//! - 5 段按「分 时 日 月 周」解释，6 段在首位补秒，7 段再补年份。
//! - `?` 等价于 `*`，但在「日/周是否通配」的判定里 **不算通配**（`0 0 ? * 1` 每天命中）。
//! - `日` 与 `周` 都受限时取 **或**；只受限一个时按 **与**（另一个通配）。
//! - 支持 `L`（月末）、`LW`（月末工作日）、`nW`（距 n 最近的工作日）、`nL`（当月最后一个周 n）、
//!   `n#k`（当月第 k 个周 n）、月份/星期英文缩写与 `@daily` 等昵称。
//! - 范围写成倒序（`50-10`）或数字前缀步长（`5/15`）视为非法。
//!
//! 所有日历计算都在「调用方给定的固定本地偏移」下进行（见 `civil` 模块注释）。

use crate::civil::{days_from_civil, days_in_month, from_local, to_local, weekday_from_days};

const MAX_SEARCH_DAYS: i64 = 366 * 1200;

#[derive(Clone, Debug)]
struct DayOfMonth {
    days: [bool; 31],
    last_day: bool,
    last_weekday: bool,
    /// 下标 = 日-1，值 = 目标日（1-31）。
    nearest: [Option<u32>; 31],
}

#[derive(Clone, Debug)]
struct DayOfWeek {
    /// 每个星期几的位掩码：bit0..bit4 = 第 1..5 次，bit5 = 最后一次。
    nth: [u8; 7],
}

#[derive(Clone, Debug)]
pub struct CronSpec {
    seconds: [bool; 60],
    minutes: [bool; 60],
    hours: [bool; 24],
    dom: DayOfMonth,
    months: [bool; 12],
    dow: DayOfWeek,
    years: Option<Vec<bool>>,
    star_dom: bool,
    star_dow: bool,
}

const NTH_ALL: u8 = 0b0001_1111;
const NTH_LAST: u8 = 0b0010_0000;

impl CronSpec {
    pub fn parse(pattern: &str) -> Result<Self, String> {
        let pattern = expand_nickname(pattern.trim())?;
        let fields: Vec<&str> = pattern.split_whitespace().collect();
        if fields.len() < 5 || fields.len() > 7 {
            return Err(format!(
                "CronPattern: invalid configuration format ('{pattern}'), exactly five, six, or seven space separated parts are required."
            ));
        }
        let mut parts: Vec<String> = fields.iter().map(|field| (*field).to_string()).collect();
        if parts.len() == 5 {
            parts.insert(0, "0".to_string());
        }
        if parts.len() == 6 {
            parts.push("*".to_string());
        }

        // croner 先处理 L/LW，再判定通配，最后才把 `?` 换成 `*`。
        let mut dom_field = parts[3].to_uppercase();
        let mut last_day = false;
        let mut last_weekday = false;
        if dom_field == "LW" {
            last_weekday = true;
            dom_field.clear();
        } else if dom_field.contains('L') {
            dom_field = dom_field.replace('L', "");
            last_day = true;
        }
        let star_dom = dom_field == "*";
        let star_dow = parts[5] == "*";
        let star_year = parts[6] == "*";

        parts[4] = replace_month_names(&parts[4]).to_uppercase();
        parts[5] = replace_day_names(&parts[5]);
        let dom_field = dom_field.to_uppercase();
        parts[3] = dom_field;

        let seconds = parse_numeric_field(&parts[0], 0, 60, "second")?;
        let minutes = parse_numeric_field(&parts[1], 0, 60, "minute")?;
        let hours = parse_numeric_field(&parts[2], 0, 24, "hour")?;
        let dom = parse_day_of_month(&parts[3], last_day, last_weekday)?;
        let months = parse_numeric_field(&parts[4], -1, 12, "month")?;
        let dow = parse_day_of_week(&parts[5])?;
        let years = if star_year {
            None
        } else {
            let mask = parse_numeric_field(&parts[6], -1, 9999, "year")?;
            // 年份是稀疏集合，按位图扩容到出现的最大年份。
            let max_year = (1..=9999).filter(|year| mask[year - 1]).max().unwrap_or(0);
            let mut bits = vec![false; max_year + 1];
            for (index, present) in mask.iter().enumerate() {
                if *present {
                    bits[index + 1] = true;
                }
            }
            Some(bits)
        };

        Ok(Self {
            seconds: to_fixed::<60>(&seconds),
            minutes: to_fixed::<60>(&minutes),
            hours: to_fixed::<24>(&hours),
            dom,
            months: to_fixed::<12>(&months),
            dow,
            years,
            star_dom,
            star_dow,
        })
    }

    fn year_matches(&self, year: i64) -> bool {
        match &self.years {
            None => true,
            Some(bits) => {
                year >= 0 && (year as usize) < bits.len() && bits[year as usize]
            }
        }
    }

    fn day_matches(&self, year: i64, month: u32, day: u32) -> bool {
        if !self.year_matches(year) || !self.months[(month - 1) as usize] {
            return false;
        }
        let dom_match = self.dom_matches(year, month, day);
        let dow_match = self.dow_matches(year, month, day);
        if self.star_dow {
            dom_match
        } else if self.star_dom {
            dow_match
        } else {
            dom_match || dow_match
        }
    }

    fn dom_matches(&self, year: i64, month: u32, day: u32) -> bool {
        let last = days_in_month(year, month);
        if self.dom.last_day && day == last {
            return true;
        }
        if self.dom.last_weekday && day == last_weekday_of_month(year, month) {
            return true;
        }
        for target in self.dom.nearest.iter().flatten() {
            if nearest_weekday(year, month, *target) == Some(day) {
                return true;
            }
        }
        self.dom.days[(day - 1) as usize]
    }

    fn dow_matches(&self, year: i64, month: u32, day: u32) -> bool {
        let weekday = weekday_from_days(days_from_civil(year, month, day));
        let mask = self.dow.nth[weekday as usize];
        if mask == 0 {
            return false;
        }
        if mask & NTH_ALL != 0 {
            let occurrence = (day - 1) / 7 + 1;
            if occurrence <= 5 && mask & (1 << (occurrence - 1)) != 0 {
                return true;
            }
        }
        if mask & NTH_LAST != 0 {
            let last = days_in_month(year, month);
            let last_weekday = weekday_from_days(days_from_civil(year, month, last));
            let last_same = last - (last_weekday + 7 - weekday) % 7;
            return day == last_same;
        }
        false
    }
}

fn to_fixed<const N: usize>(source: &[bool]) -> [bool; N] {
    let mut out = [false; N];
    for (index, value) in source.iter().take(N).enumerate() {
        out[index] = *value;
    }
    out
}

fn expand_nickname(pattern: &str) -> Result<String, String> {
    if !pattern.contains('@') {
        return Ok(pattern.to_string());
    }
    let lower = pattern.trim().to_lowercase();
    let expanded = match lower.as_str() {
        "@yearly" | "@annually" => "0 0 1 1 *",
        "@monthly" => "0 0 1 * *",
        "@weekly" => "0 0 * * 0",
        "@daily" | "@midnight" => "0 0 * * *",
        "@hourly" => "0 * * * *",
        "@reboot" => return Err("CronPattern: @reboot is not supported".to_string()),
        _ => pattern,
    };
    Ok(expanded.to_string())
}

fn replace_month_names(field: &str) -> String {
    let mut out = field.to_lowercase();
    for (index, name) in ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"]
        .iter()
        .enumerate()
    {
        out = out.replace(name, &(index + 1).to_string());
    }
    out
}

fn replace_day_names(field: &str) -> String {
    let mut out = field.to_lowercase();
    // croner 先把 `-sun` 归一到 7，再替换其余缩写，避免 SUN 单独出现时被当成周日之外的值。
    out = out.replace("-sun", "-7");
    for (index, name) in ["sun", "mon", "tue", "wed", "thu", "fri", "sat"].iter().enumerate() {
        out = out.replace(name, &index.to_string());
    }
    out
}

/// 解析普通数值字段，返回长度 `length` 的布尔表。
///
/// `offset` 是「书写数值 → 数组下标」的位移：日/月为 -1（1 起），其余为 0。
fn parse_numeric_field(
    field: &str,
    offset: i64,
    length: usize,
    name: &str,
) -> Result<Vec<bool>, String> {
    let field = field.replace('?', "*");
    let mut mask = vec![false; length];
    if field == "*" {
        mask.fill(true);
        return Ok(mask);
    }
    for item in field.split(',') {
        let item = item.trim();
        if item.is_empty() {
            return Err(format!("CronPattern: configuration entry {name} is empty"));
        }
        let (head, step) = split_step(item)?;
        if step > length as i64 {
            return Err(format!("CronPattern: Syntax error, illegal stepping: '{item}'"));
        }
        let (from, to) = if head == "*" {
            (-offset, (length as i64 - 1) - offset)
        } else {
            split_range(head)?
        };
        let mut cursor = from;
        while cursor <= to {
            let index = cursor + offset;
            if index < 0 || index as usize >= length {
                return Err(format!("CronPattern: Invalid value for {name}: {cursor}"));
            }
            mask[index as usize] = true;
            cursor += step;
        }
    }
    Ok(mask)
}

/// 拆出步长：`*`/`*​/step`/`a-b/step`；数字前缀步长按 croner 视为非法。
fn split_step(item: &str) -> Result<(&str, i64), String> {
    let Some((head, step)) = item.split_once('/') else {
        return Ok((item, 1));
    };
    if head.is_empty() {
        return Err("CronPattern: stepping with missing prefix".to_string());
    }
    if head != "*" && !head.contains('-') {
        return Err(format!(
            "CronPattern: Syntax error, stepping with numeric prefix ('{item}') is not allowed."
        ));
    }
    let step: i64 = step
        .parse()
        .map_err(|_| format!("CronPattern: Syntax error, illegal stepping: '{item}'"))?;
    if step <= 0 {
        return Err(format!("CronPattern: Syntax error, illegal stepping: '{item}'"));
    }
    Ok((head, step))
}

fn split_range(range: &str) -> Result<(i64, i64), String> {
    if let Some((from, to)) = range.split_once('-') {
        let from: i64 = from
            .parse()
            .map_err(|_| format!("CronPattern: Syntax error, illegal range: '{range}'"))?;
        let to: i64 = to
            .parse()
            .map_err(|_| format!("CronPattern: Syntax error, illegal range: '{range}'"))?;
        if from > to {
            return Err(format!(
                "CronPattern: From value is larger than to value: '{range}'"
            ));
        }
        return Ok((from, to));
    }
    let value: i64 = range
        .parse()
        .map_err(|_| format!("CronPattern: {range} is not a number"))?;
    Ok((value, value))
}

fn parse_day_of_month(field: &str, last_day: bool, last_weekday: bool) -> Result<DayOfMonth, String> {
    let field = field.replace('?', "*");
    let mut dom = DayOfMonth {
        days: [false; 31],
        last_day,
        last_weekday,
        nearest: [None; 31],
    };
    if field.is_empty() {
        return Ok(dom);
    }
    if field == "*" {
        dom.days.fill(true);
        return Ok(dom);
    }
    for item in field.split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        if item.to_uppercase().contains('W') {
            let day: i64 = item
                .trim_end_matches(['W', 'w'])
                .parse()
                .map_err(|_| format!("CronPattern: nearest weekday is not a number: '{item}'"))?;
            if !(1..=31).contains(&day) {
                return Err(format!("CronPattern: Invalid value for day: {}", day - 1));
            }
            dom.nearest[(day - 1) as usize] = Some(day as u32);
            continue;
        }
        let (head, step) = split_step(item)?;
        if step > 31 {
            return Err(format!("CronPattern: Syntax error, illegal stepping: '{item}'"));
        }
        let (from, to) = if head == "*" { (1, 31) } else { split_range(head)? };
        let mut cursor = from;
        while cursor <= to {
            if !(1..=31).contains(&cursor) {
                return Err(format!("CronPattern: Invalid value for day: {}", cursor - 1));
            }
            dom.days[(cursor - 1) as usize] = true;
            cursor += step;
        }
    }
    Ok(dom)
}

fn parse_day_of_week(field: &str) -> Result<DayOfWeek, String> {
    let field = field.replace('?', "*");
    let mut dow = DayOfWeek { nth: [0; 7] };
    if field == "*" {
        dow.nth = [NTH_ALL; 7];
        return Ok(dow);
    }
    for item in field.split(',') {
        let mut item = item.trim().to_string();
        if item.is_empty() {
            continue;
        }
        let mut modifier: Option<String> = None;
        if let Some((head, nth)) = item.split_once('#') {
            modifier = Some(format!("#{nth}"));
            item = head.to_string();
        } else if item.to_uppercase().ends_with('L') {
            modifier = Some("L".to_string());
            item = item[..item.len() - 1].to_string();
        }
        let (head, step) = split_step(&item)?;
        if step > 7 {
            return Err(format!("CronPattern: Syntax error, illegal stepping: '{item}'"));
        }
        let (from, to) = if head == "*" { (0, 6) } else { split_range(head)? };
        // croner 把 7 也当作周日。
        let mask = match modifier.as_deref() {
            None => NTH_ALL,
            Some("L") => NTH_LAST,
            Some(value) if value.starts_with('#') => {
                let nth: u8 = value[1..]
                    .parse()
                    .map_err(|_| format!("CronPattern: nth weekday out of range: '{value}'"))?;
                if !(1..=5).contains(&nth) {
                    return Err(format!(
                        "CronPattern: nth weekday out of range, should be 1-5 or L. Value: {nth}"
                    ));
                }
                1 << (nth - 1)
            }
            Some(other) => return Err(format!("CronPattern: invalid day-of-week modifier: {other}")),
        };
        let mut cursor = from;
        while cursor <= to {
            let weekday = if cursor == 7 { 0 } else { cursor };
            if !(0..=7).contains(&weekday) {
                return Err(format!("CronPattern: Invalid value for dayOfWeek: {cursor}"));
            }
            dow.nth[weekday as usize] |= mask;
            cursor += step;
        }
    }
    Ok(dow)
}

fn last_weekday_of_month(year: i64, month: u32) -> u32 {
    let last = days_in_month(year, month);
    let weekday = weekday_from_days(days_from_civil(year, month, last));
    match weekday {
        0 => last - 2,
        6 => last - 1,
        _ => last,
    }
}

fn nearest_weekday(year: i64, month: u32, day: u32) -> Option<u32> {
    let last = days_in_month(year, month);
    if day > last {
        return None;
    }
    let weekday = weekday_from_days(days_from_civil(year, month, day));
    Some(match weekday {
        0 => {
            if day == last {
                day - 2
            } else {
                day + 1
            }
        }
        6 => {
            if day == 1 {
                day + 2
            } else {
                day - 1
            }
        }
        _ => day,
    })
}

fn floor_to_second(ms: i64) -> i64 {
    ms.div_euclid(1000) * 1000
}

/// 严格晚于 `from_ms` 的下一次触发（本地偏移固定为 `offset_secs`）。
pub fn next_run(spec: &CronSpec, from_ms: i64, offset_secs: i32) -> Option<i64> {
    let start = floor_to_second(from_ms) + 1000;
    let start_local = to_local(start, offset_secs);
    let start_days = days_from_civil(start_local.year, start_local.month, start_local.day);
    for day_index in 0..=MAX_SEARCH_DAYS {
        let days = start_days + day_index;
        let (year, month, day) = crate::civil::civil_from_days(days);
        if !spec.day_matches(year, month, day) {
            continue;
        }
        let midnight = from_local(year, month as i64, day as i64, 0, 0, offset_secs);
        let lower_bound = if day_index == 0 { start } else { i64::MIN };
        for hour in 0..24usize {
            if !spec.hours[hour] {
                continue;
            }
            for minute in 0..60usize {
                if !spec.minutes[minute] {
                    continue;
                }
                for second in 0..60usize {
                    if !spec.seconds[second] {
                        continue;
                    }
                    let candidate =
                        midnight + (hour as i64) * 3_600_000 + (minute as i64) * 60_000 + second as i64 * 1000;
                    if candidate >= lower_bound && candidate >= start {
                        return Some(candidate);
                    }
                }
            }
        }
    }
    None
}

/// 严格早于 `from_ms` 的最近 `count` 次触发（由近到远）。
pub fn previous_runs(spec: &CronSpec, from_ms: i64, count: usize, offset_secs: i32) -> Vec<i64> {
    let mut out = Vec::new();
    if count == 0 {
        return out;
    }
    let mut cursor = floor_to_second(from_ms) - 1000;
    for _ in 0..MAX_SEARCH_DAYS {
        let local = to_local(cursor, offset_secs);
        let day_start = from_local(local.year, local.month as i64, local.day as i64, 0, 0, offset_secs);
        if spec.day_matches(local.year, local.month, local.day) {
            let mut found = prior_in_day(spec, day_start, cursor);
            while let Some(candidate) = found {
                out.push(candidate);
                if out.len() == count {
                    return out;
                }
                found = prior_in_day(spec, day_start, candidate - 1000);
            }
        }
        cursor = day_start - 1000;
    }
    out
}

fn prior_in_day(spec: &CronSpec, day_start: i64, upper_bound: i64) -> Option<i64> {
    for hour in (0..24usize).rev() {
        if !spec.hours[hour] {
            continue;
        }
        for minute in (0..60usize).rev() {
            if !spec.minutes[minute] {
                continue;
            }
            for second in (0..60usize).rev() {
                if !spec.seconds[second] {
                    continue;
                }
                let candidate =
                    day_start + (hour as i64) * 3_600_000 + (minute as i64) * 60_000 + second as i64 * 1000;
                if candidate <= upper_bound {
                    return Some(candidate);
                }
            }
        }
    }
    None
}
