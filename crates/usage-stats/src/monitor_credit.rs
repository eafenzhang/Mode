//! BigModel credit-usage monitor → CodingPlanUsageSnapshot 投影。
//!
//! 真源：`packages/services/src/usage-stats/providers/bigmodelUsageMonitorMapper.ts`
//! 的 `buildCodingPlanUsageSnapshotFromMonitor` 及其归一化 helper。
//! 只做纯 JSON 投影；`generatedAt` 与 xTime/granularity 由调用方注入。

use regex::Regex;
use serde_json::{json, Number, Value};

fn number(value: f64) -> Value {
    if value.is_finite() && value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_992.0 {
        Value::Number(Number::from(value as i64))
    } else {
        Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

fn optional_number(value: Option<f64>) -> Value {
    value.map(number).unwrap_or(Value::Null)
}

fn to_finite_number(value: Option<&Value>) -> Option<f64> {
    let value = value?;
    let numeric = match value {
        Value::Number(number) => number.as_f64()?,
        Value::String(text) if !text.trim().is_empty() => text.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    numeric.is_finite().then_some(numeric)
}

fn normalize_number_series(values: Option<&Value>) -> Vec<f64> {
    values
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(|value| to_finite_number(Some(value)).unwrap_or(0.0))
                .collect()
        })
        .unwrap_or_default()
}

fn sum_numbers(values: &[f64]) -> f64 {
    values.iter().sum()
}

fn sum_series(series_list: &[Option<Vec<f64>>]) -> Option<Vec<f64>> {
    let max_length = series_list
        .iter()
        .map(|series| series.as_ref().map_or(0, Vec::len))
        .max()
        .unwrap_or(0);
    if max_length == 0 {
        return None;
    }
    Some(
        (0..max_length)
            .map(|index| {
                series_list
                    .iter()
                    .map(|series| {
                        series
                            .as_ref()
                            .and_then(|series| series.get(index).copied())
                            .unwrap_or(0.0)
                    })
                    .sum()
            })
            .collect(),
    )
}

fn pick_first_non_empty_series(series_list: &[Option<Vec<f64>>]) -> Option<Vec<f64>> {
    series_list
        .iter()
        .flatten()
        .find(|series| series.iter().any(|value| *value > 0.0))
        .cloned()
}

fn normalize_axis_value(value: &str, granularity: &str) -> String {
    if granularity == "hour" {
        let time_pattern = Regex::new(r"(\d{2}):(\d{2})(?::(\d{2}))?").expect("时间轴正则可编译");
        if let Some(captures) = time_pattern.captures(value) {
            return format!(
                "{}:{}:{}",
                &captures[1],
                &captures[2],
                captures.get(3).map_or("00", |value| value.as_str())
            );
        }
    }
    let date_pattern = Regex::new(r"(\d{4})-(\d{2})-(\d{2})").expect("日期轴正则可编译");
    if let Some(captures) = date_pattern.captures(value) {
        return format!("{}-{}-{}", &captures[1], &captures[2], &captures[3]);
    }
    value.to_string()
}

fn align_usage_series(
    source_x_time: &[Value],
    target_x_time: &[Value],
    values: Option<&Value>,
    granularity: &str,
) -> Vec<f64> {
    let source_values = normalize_number_series(values);
    if source_x_time.is_empty() && source_values.len() == target_x_time.len() {
        return source_values;
    }
    let mut values_by_time = std::collections::HashMap::new();
    for (index, time) in source_x_time.iter().enumerate() {
        if let Some(time) = time.as_str() {
            values_by_time.insert(
                normalize_axis_value(time, granularity),
                source_values.get(index).copied().unwrap_or(0.0),
            );
        }
    }
    target_x_time
        .iter()
        .enumerate()
        .map(|(index, time)| {
            let normalized = time
                .as_str()
                .map(|time| normalize_axis_value(time, granularity))
                .unwrap_or_default();
            values_by_time.get(&normalized).copied().unwrap_or_else(|| {
                if source_x_time.is_empty() {
                    source_values.get(index).copied().unwrap_or(0.0)
                } else {
                    0.0
                }
            })
        })
        .collect()
}

fn array(value: Option<&Value>) -> &[Value] {
    value
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn object(value: Option<&Value>) -> Option<&serde_json::Map<String, Value>> {
    value.and_then(Value::as_object)
}

fn series_at<'a>(object: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a Value> {
    object.get(key).filter(|value| value.is_array())
}

fn number_at(object: &serde_json::Map<String, Value>, key: &str) -> Option<f64> {
    to_finite_number(object.get(key))
}

fn number_or_zero(object: &serde_json::Map<String, Value>, key: &str) -> f64 {
    number_at(object, key).unwrap_or(0.0)
}

fn normalize_model_data_list(
    payload: &serde_json::Map<String, Value>,
    target_x_time: &[Value],
    granularity: &str,
) -> Vec<Value> {
    let source_x_time = array(payload.get("x_time"));
    let filtered: Vec<&serde_json::Map<String, Value>> = array(payload.get("modelDataList"))
        .iter()
        .filter_map(|item| item.as_object())
        .filter(|item| {
            item.get("modelName")
                .and_then(Value::as_str)
                .is_some_and(|name| !name.trim().is_empty())
        })
        .collect();
    let mut rows: Vec<(f64, Value)> = filtered
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            let model_name = item["modelName"].as_str().unwrap_or("").trim();
            let sort_order = number_at(item, "sortOrder").unwrap_or(index as f64);
            (
                sort_order,
                json!({
                    "modelName": model_name,
                    "sortOrder": number(sort_order),
                    "tokensUsage": align_usage_series(source_x_time, target_x_time, series_at(item, "tokensUsage"), granularity).iter().map(|value| number(*value)).collect::<Vec<_>>(),
                    "totalTokens": number(number_or_zero(item, "totalTokens")),
                }),
            )
        })
        .collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    rows.into_iter().map(|(_, value)| value).collect()
}

fn normalize_model_summary_list(payload: &serde_json::Map<String, Value>) -> Vec<Value> {
    let list = payload
        .get("modelSummaryList")
        .filter(|value| value.is_array())
        .or_else(|| {
            object(payload.get("totalUsage"))
                .and_then(|total| total.get("modelSummaryList"))
                .filter(|value| value.is_array())
        });
    let filtered: Vec<&serde_json::Map<String, Value>> = array(list)
        .iter()
        .filter_map(|item| item.as_object())
        .filter(|item| {
            item.get("modelName")
                .and_then(Value::as_str)
                .is_some_and(|name| !name.trim().is_empty())
        })
        .collect();
    let mut rows: Vec<(f64, Value)> = filtered
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            let model_name = item["modelName"].as_str().unwrap_or("").trim();
            let sort_order = number_at(item, "sortOrder").unwrap_or(index as f64);
            (
                sort_order,
                json!({
                    "modelName": model_name,
                    "totalTokens": number(number_or_zero(item, "totalTokens")),
                    "sortOrder": number(sort_order),
                }),
            )
        })
        .collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    rows.into_iter().map(|(_, value)| value).collect()
}

fn is_credit_usage_breakdown_bucket(item: &serde_json::Map<String, Value>) -> bool {
    let code = item.get("modelCode").and_then(Value::as_str).map(str::trim);
    if let Some(code) = code {
        if matches!(
            code,
            "cached_input"
                | "cachedInput"
                | "cache_input"
                | "cacheInput"
                | "uncached_input"
                | "uncachedInput"
                | "output"
                | "output_tokens"
                | "outputTokens"
        ) {
            return true;
        }
    }
    let name = item
        .get("modelName")
        .and_then(Value::as_str)
        .map(|name| name.trim().to_lowercase());
    matches!(name.as_deref(), Some("缓存" | "未缓存" | "输出"))
}

fn normalize_credit_model_data_list(
    payload: &serde_json::Map<String, Value>,
    target_x_time: &[Value],
    granularity: &str,
) -> Vec<Value> {
    let source_x_time = array(payload.get("xTime"));
    let filtered: Vec<&serde_json::Map<String, Value>> = array(payload.get("modelDataList"))
        .iter()
        .filter_map(|item| item.as_object())
        .filter(|item| {
            item.get("modelName")
                .and_then(Value::as_str)
                .is_some_and(|name| !name.trim().is_empty())
                || item
                    .get("modelCode")
                    .and_then(Value::as_str)
                    .is_some_and(|code| !code.trim().is_empty())
        })
        .filter(|item| !is_credit_usage_breakdown_bucket(item))
        .collect();
    let mut rows: Vec<(f64, Value)> = filtered
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            let series = |key: &str| series_at(item, key);
            let cached_input_tokens = align_usage_series(
                source_x_time,
                target_x_time,
                series("cachedInputTokensUsage"),
                granularity,
            );
            let uncached_input_tokens = align_usage_series(
                source_x_time,
                target_x_time,
                series("uncachedInputTokensUsage"),
                granularity,
            );
            let output_tokens = align_usage_series(
                source_x_time,
                target_x_time,
                series("outputTokensUsage"),
                granularity,
            );
            let cached_input_credits = align_usage_series(
                source_x_time,
                target_x_time,
                series("cachedInputCreditsUsage"),
                granularity,
            );
            let uncached_input_credits = align_usage_series(
                source_x_time,
                target_x_time,
                series("uncachedInputCreditsUsage"),
                granularity,
            );
            let output_credits = align_usage_series(
                source_x_time,
                target_x_time,
                series("outputCreditsUsage"),
                granularity,
            );
            let token_series = pick_first_non_empty_series(&[
                series("totalTokensUsage").map(|value| normalize_number_series(Some(value))),
                series("tokensUsage").map(|value| normalize_number_series(Some(value))),
                sum_series(&[
                    series("cachedInputTokensUsage")
                        .map(|value| normalize_number_series(Some(value))),
                    series("uncachedInputTokensUsage")
                        .map(|value| normalize_number_series(Some(value))),
                    series("outputTokensUsage")
                        .map(|value| normalize_number_series(Some(value))),
                ]),
            ])
            .unwrap_or_default();
            let credit_series = pick_first_non_empty_series(&[
                series("totalCreditsUsage").map(|value| normalize_number_series(Some(value))),
                sum_series(&[
                    series("cachedInputCreditsUsage")
                        .map(|value| normalize_number_series(Some(value))),
                    series("uncachedInputCreditsUsage")
                        .map(|value| normalize_number_series(Some(value))),
                    series("outputCreditsUsage")
                        .map(|value| normalize_number_series(Some(value))),
                ]),
            ])
            .unwrap_or_default();
            let model_name = item
                .get("modelName")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    item.get("modelCode")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                })
                .unwrap_or("unknown");
            let sort_order = number_at(item, "sortOrder").unwrap_or(index as f64);
            let total_tokens =
                number_at(item, "totalTokens").unwrap_or_else(|| sum_numbers(&token_series));
            let total_credits =
                number_at(item, "totalCredits").unwrap_or_else(|| sum_numbers(&credit_series));
            (
                sort_order,
                json!({
                    "modelName": model_name,
                    "sortOrder": number(sort_order),
                    "tokensUsage": align_usage_series(source_x_time, target_x_time, Some(&json!(token_series)), granularity).iter().map(|value| number(*value)).collect::<Vec<_>>(),
                    "creditsUsage": align_usage_series(source_x_time, target_x_time, Some(&json!(credit_series)), granularity).iter().map(|value| number(*value)).collect::<Vec<_>>(),
                    "cachedInputTokensUsage": cached_input_tokens.iter().map(|value| number(*value)).collect::<Vec<_>>(),
                    "uncachedInputTokensUsage": uncached_input_tokens.iter().map(|value| number(*value)).collect::<Vec<_>>(),
                    "outputTokensUsage": output_tokens.iter().map(|value| number(*value)).collect::<Vec<_>>(),
                    "cachedInputCreditsUsage": cached_input_credits.iter().map(|value| number(*value)).collect::<Vec<_>>(),
                    "uncachedInputCreditsUsage": uncached_input_credits.iter().map(|value| number(*value)).collect::<Vec<_>>(),
                    "outputCreditsUsage": output_credits.iter().map(|value| number(*value)).collect::<Vec<_>>(),
                    "totalTokens": number(total_tokens),
                    "totalCredits": number(total_credits),
                }),
            )
        })
        .collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    rows.into_iter().map(|(_, value)| value).collect()
}

fn normalize_credit_model_summary_list(payload: &serde_json::Map<String, Value>) -> Vec<Value> {
    let x_time = array(payload.get("xTime"));
    let granularity = if x_time.iter().filter_map(Value::as_str).any(|value| {
        Regex::new(r"\d{2}:\d{2}")
            .expect("时刻正则可编译")
            .is_match(value)
    }) {
        "hour"
    } else {
        "day"
    };
    normalize_credit_model_data_list(payload, x_time, granularity)
        .into_iter()
        .map(|item| {
            json!({
                "modelName": item["modelName"],
                "totalTokens": item["totalTokens"],
                "totalCredits": item["totalCredits"],
                "sortOrder": item["sortOrder"],
            })
        })
        .collect()
}

fn sum_credit_model_tokens(payload: &serde_json::Map<String, Value>) -> f64 {
    let items: Vec<&serde_json::Map<String, Value>> = array(payload.get("modelDataList"))
        .iter()
        .filter_map(|item| item.as_object())
        .collect();
    let has_real_model_row = items
        .iter()
        .any(|item| !is_credit_usage_breakdown_bucket(item));
    let effective: Vec<&serde_json::Map<String, Value>> = if has_real_model_row {
        items
            .into_iter()
            .filter(|item| !is_credit_usage_breakdown_bucket(item))
            .collect()
    } else {
        items
    };
    let mut total = 0.0;
    for item in effective {
        let token_series = pick_first_non_empty_series(&[
            item.get("totalTokensUsage")
                .map(|value| normalize_number_series(Some(value))),
            item.get("tokensUsage")
                .map(|value| normalize_number_series(Some(value))),
            sum_series(&[
                item.get("cachedInputTokensUsage")
                    .map(|value| normalize_number_series(Some(value))),
                item.get("uncachedInputTokensUsage")
                    .map(|value| normalize_number_series(Some(value))),
                item.get("outputTokensUsage")
                    .map(|value| normalize_number_series(Some(value))),
            ]),
        ])
        .unwrap_or_default();
        total += number_at(item, "totalTokens").unwrap_or_else(|| sum_numbers(&token_series));
    }
    total
}

fn normalize_tool_data_list(
    payload: &serde_json::Map<String, Value>,
    target_x_time: &[Value],
    granularity: &str,
) -> Vec<Value> {
    let source_x_time = array(payload.get("x_time"));
    let filtered: Vec<&serde_json::Map<String, Value>> = array(payload.get("toolDataList"))
        .iter()
        .filter_map(|item| item.as_object())
        .filter(|item| {
            item.get("toolCode")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty())
                || item
                    .get("toolName")
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.trim().is_empty())
        })
        .collect();
    let mut rows: Vec<(f64, Value)> = filtered
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            let code = item
                .get("toolCode")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    item.get("toolName")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                })
                .unwrap_or("unknown");
            let name = item
                .get("toolName")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    item.get("toolCode")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                })
                .unwrap_or("Unknown");
            let sort_order = number_at(item, "sortOrder").unwrap_or(index as f64);
            (
                sort_order,
                json!({
                    "toolCode": code,
                    "toolName": name,
                    "sortOrder": number(sort_order),
                    "usageCount": align_usage_series(source_x_time, target_x_time, series_at(item, "usageCount"), granularity).iter().map(|value| number(*value)).collect::<Vec<_>>(),
                    "totalUsageCount": number(number_or_zero(item, "totalUsageCount")),
                }),
            )
        })
        .collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    rows.into_iter().map(|(_, value)| value).collect()
}

fn normalize_tool_summary_list(payload: &serde_json::Map<String, Value>) -> Vec<Value> {
    let list = payload
        .get("toolSummaryList")
        .filter(|value| value.is_array())
        .or_else(|| {
            object(payload.get("totalUsage"))
                .and_then(|total| total.get("toolSummaryList"))
                .filter(|value| value.is_array())
        });
    let filtered: Vec<&serde_json::Map<String, Value>> = array(list)
        .iter()
        .filter_map(|item| item.as_object())
        .filter(|item| {
            item.get("toolCode")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty())
                || item
                    .get("toolName")
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.trim().is_empty())
        })
        .collect();
    let mut rows: Vec<(f64, Value)> = filtered
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            let code = item
                .get("toolCode")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    item.get("toolName")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                })
                .unwrap_or("unknown");
            let name = item
                .get("toolName")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    item.get("toolCode")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                })
                .unwrap_or("Unknown");
            let sort_order = number_at(item, "sortOrder").unwrap_or(index as f64);
            (
                sort_order,
                json!({
                    "toolCode": code,
                    "toolName": name,
                    "totalUsageCount": number(number_or_zero(item, "totalUsageCount")),
                    "sortOrder": number(sort_order),
                }),
            )
        })
        .collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    rows.into_iter().map(|(_, value)| value).collect()
}

fn normalize_credit_tool_data_list(
    payload: &serde_json::Map<String, Value>,
    target_x_time: &[Value],
    granularity: &str,
) -> Vec<Value> {
    let source_x_time = array(payload.get("xTime"));
    let filtered: Vec<&serde_json::Map<String, Value>> = array(payload.get("mcpDataList"))
        .iter()
        .filter_map(|item| item.as_object())
        .filter(|item| {
            ["mcpCode", "mcpName", "toolCode", "toolName"]
                .iter()
                .any(|key| {
                    item.get(*key)
                        .and_then(Value::as_str)
                        .is_some_and(|value| !value.trim().is_empty())
                })
        })
        .collect();
    let mut rows: Vec<(f64, Value)> = filtered
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            let usage_series = pick_first_non_empty_series(&[
                item.get("mcpCallCount")
                    .map(|value| normalize_number_series(Some(value))),
                item.get("usageCount")
                    .map(|value| normalize_number_series(Some(value))),
            ])
            .unwrap_or_default();
            let credit_series = normalize_number_series(item.get("creditsUsage"));
            let text = |key: &str| {
                item.get(key)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
            };
            let code = text("mcpCode")
                .or_else(|| text("toolCode"))
                .or_else(|| text("mcpName"))
                .unwrap_or("unknown");
            let name = text("mcpName")
                .or_else(|| text("toolName"))
                .or_else(|| text("mcpCode"))
                .unwrap_or(code);
            let sort_order = number_at(item, "sortOrder").unwrap_or(index as f64);
            let total_usage =
                number_at(item, "totalUsageCount").unwrap_or_else(|| sum_numbers(&usage_series));
            let total_credits =
                number_at(item, "totalCredits").unwrap_or_else(|| sum_numbers(&credit_series));
            (
                sort_order,
                json!({
                    "toolCode": code,
                    "toolName": name,
                    "sortOrder": number(sort_order),
                    "usageCount": align_usage_series(source_x_time, target_x_time, Some(&json!(usage_series)), granularity).iter().map(|value| number(*value)).collect::<Vec<_>>(),
                    "creditsUsage": align_usage_series(source_x_time, target_x_time, Some(&json!(credit_series)), granularity).iter().map(|value| number(*value)).collect::<Vec<_>>(),
                    "totalUsageCount": number(total_usage),
                    "totalCredits": number(total_credits),
                }),
            )
        })
        .collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    rows.into_iter().map(|(_, value)| value).collect()
}

fn normalize_credit_tool_summary_list(payload: &serde_json::Map<String, Value>) -> Vec<Value> {
    let x_time = array(payload.get("xTime"));
    let granularity = if x_time.iter().filter_map(Value::as_str).any(|value| {
        Regex::new(r"\d{2}:\d{2}")
            .expect("时刻正则可编译")
            .is_match(value)
    }) {
        "hour"
    } else {
        "day"
    };
    normalize_credit_tool_data_list(payload, x_time, granularity)
        .into_iter()
        .map(|item| {
            json!({
                "toolCode": item["toolCode"],
                "toolName": item["toolName"],
                "totalUsageCount": item["totalUsageCount"],
                "totalCredits": item["totalCredits"],
                "sortOrder": item["sortOrder"],
            })
        })
        .collect()
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

fn date_key_to_day_index(date_key: &str) -> Option<i64> {
    let pattern = Regex::new(r"^(\d{4})-(\d{2})-(\d{2})$").ok()?;
    let captures = pattern.captures(date_key)?;
    let year = captures[1].parse::<i64>().ok()?;
    let month = captures[2].parse::<u32>().ok()?;
    let day = captures[3].parse::<u32>().ok()?;
    if !(1..=12).contains(&month) {
        return None;
    }
    Some(days_from_civil(year, month, 1) + day as i64 - 1)
}

fn utc_day_index_to_date_key(day_index: i64) -> String {
    let (year, month, day) = civil_from_days(day_index);
    format!("{year:04}-{month:02}-{day:02}")
}

fn level_for_tokens(value: f64, max: f64) -> u32 {
    if value <= 0.0 || max <= 0.0 {
        return 0;
    }
    ((value / max) * 4.0).ceil().clamp(1.0, 4.0) as u32
}

fn build_activity_heatmap(series: &Value) -> Value {
    let mut cells: Vec<Value> = array(Some(series))
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|item| {
            let date = item
                .get("date")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or("");
            if !Regex::new(r"^\d{4}-\d{2}-\d{2}$")
                .expect("日期正则可编译")
                .is_match(date)
            {
                return None;
            }
            Some(json!({
                "date": date,
                "totalTokens": number(to_finite_number(item.get("totalTokens")).unwrap_or(0.0)),
                "turnCount": number(to_finite_number(item.get("modelCallCount")).unwrap_or(0.0)),
                "toolCallCount": number(to_finite_number(item.get("mcpCalls")).unwrap_or(0.0)),
            }))
        })
        .collect();
    cells.sort_by(|a, b| {
        a["date"]
            .as_str()
            .unwrap_or("")
            .cmp(b["date"].as_str().unwrap_or(""))
    });
    let max_tokens = cells
        .iter()
        .map(|cell| cell["totalTokens"].as_f64().unwrap_or(0.0))
        .fold(0.0_f64, f64::max);
    let by_date: std::collections::HashMap<String, Value> = cells
        .iter()
        .filter_map(|cell| Some((cell["date"].as_str()?.to_string(), cell.clone())))
        .collect();
    let mut week_starts = std::collections::BTreeSet::new();
    for cell in &cells {
        let Some(day_index) = date_key_to_day_index(cell["date"].as_str().unwrap_or("")) else {
            continue;
        };
        let weekday = (day_index + 4).rem_euclid(7);
        week_starts.insert(day_index - weekday);
    }
    let weeks: Vec<Value> = week_starts
        .into_iter()
        .enumerate()
        .map(|(week_index, week_start)| {
            let days: Vec<Value> = (0..7)
                .map(|offset| {
                    let date = utc_day_index_to_date_key(week_start + offset);
                    match by_date.get(&date) {
                        Some(source) => {
                            let mut cell = source.clone();
                            cell["level"] = json!(level_for_tokens(
                                source["totalTokens"].as_f64().unwrap_or(0.0),
                                max_tokens
                            ));
                            cell
                        }
                        None => Value::Null,
                    }
                })
                .collect();
            json!({ "weekIndex": week_index, "days": days })
        })
        .collect();
    json!({
        "startDate": cells.first().map(|cell| cell["date"].clone()).unwrap_or(Value::Null),
        "endDate": cells.last().map(|cell| cell["date"].clone()).unwrap_or(Value::Null),
        "maxTokens": number(max_tokens),
        "weeks": weeks,
    })
}

fn build_activity_snapshot(activity_data: Option<&Value>) -> Value {
    let payload = activity_data.and_then(Value::as_object);
    let summary = payload.and_then(|payload| object(payload.get("summary")));
    let series = payload
        .and_then(|payload| payload.get("series"))
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    json!({
        "summary": {
            "totalTokens": number(summary.map_or(0.0, |summary| number_or_zero(summary, "totalTokens"))),
            "peakDailyTokens": number(summary.map_or(0.0, |summary| number_or_zero(summary, "peakDailyTokens"))),
            "peakDailyTokensDate": summary
                .and_then(|summary| summary.get("peakDailyTokensDate"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| Value::String(value.to_string()))
                .unwrap_or(Value::Null),
            "totalUsageDurationMs": number(summary.map_or(0.0, |summary| number_or_zero(summary, "totalUsageDurationMs"))),
            "currentStreakDays": number(summary.map_or(0.0, |summary| number_or_zero(summary, "currentStreakDays"))),
            "longestStreakDays": number(summary.map_or(0.0, |summary| number_or_zero(summary, "longestStreakDays"))),
            "favoriteModelName": null,
        },
        "heatmap": build_activity_heatmap(&series),
    })
}

fn build_detail_summary(summary: Option<&Value>) -> Value {
    let summary = summary.and_then(Value::as_object);
    let metric = |key: &str, field: &str| {
        summary
            .and_then(|summary| object(summary.get(key)))
            .and_then(|metric| to_finite_number(metric.get(field)))
    };
    json!({
        "cacheHitRate": optional_number(metric("cacheHitRate", "value")),
        "cacheHitRateTrend": optional_number(metric("cacheHitRate", "trend")),
        "totalCredits": number(metric("totalCredits", "value").unwrap_or(0.0)),
        "totalCreditsTrend": optional_number(metric("totalCredits", "trend")),
        "averageDailyCredits": number(metric("averageDailyCredits", "value").unwrap_or(0.0)),
        "averageDailyCreditsTrend": optional_number(metric("averageDailyCredits", "trend")),
    })
}

fn build_detail_snapshot(model_detail: Option<&Value>, tool_detail: Option<&Value>) -> Value {
    json!({
        "model": build_detail_summary(
            model_detail
                .and_then(Value::as_object)
                .and_then(|detail| detail.get("summary"))
        ),
        "tool": build_detail_summary(
            tool_detail
                .and_then(Value::as_object)
                .and_then(|detail| detail.get("summary"))
        ),
    })
}

fn build_health_snapshot(health_data: Option<&Value>) -> Value {
    let payload = health_data.and_then(Value::as_object);
    let x_time = payload
        .and_then(|payload| payload.get("x_time"))
        .filter(|value| value.is_array())
        .or_else(|| {
            payload
                .and_then(|payload| payload.get("xTime"))
                .filter(|value| value.is_array())
        })
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    json!({
        "xTime": x_time,
        "proMaxDecodeSpeed": payload
            .and_then(|payload| payload.get("proMaxDecodeSpeed"))
            .map(|value| normalize_number_series(Some(value)).iter().map(|value| number(*value)).collect::<Vec<_>>())
            .unwrap_or_default(),
        "liteDecodeSpeed": payload
            .and_then(|payload| payload.get("liteDecodeSpeed"))
            .map(|value| normalize_number_series(Some(value)).iter().map(|value| number(*value)).collect::<Vec<_>>())
            .unwrap_or_default(),
    })
}

pub fn build_coding_plan_usage_snapshot_from_monitor(params: &Value, generated_at: i64) -> Value {
    let params = params.as_object().cloned().unwrap_or_default();
    let request = params.get("request").cloned().unwrap_or_else(|| json!({}));
    let provider = params.get("provider").cloned().unwrap_or(Value::Null);
    let quota = params.get("quota").cloned().unwrap_or(Value::Null);
    let granularity = params
        .get("granularity")
        .and_then(Value::as_str)
        .unwrap_or("day");
    let x_time_value = params
        .get("xTime")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    let x_time = array(Some(&x_time_value));
    let model_usage = params
        .get("modelDetailData")
        .and_then(Value::as_object)
        .and_then(|detail| detail.get("modelUsage"))
        .and_then(Value::as_object);
    let tool_usage = params
        .get("toolDetailData")
        .and_then(Value::as_object)
        .and_then(|detail| detail.get("mcpUsage"))
        .and_then(Value::as_object);
    let model_summary_list = if let Some(model_usage) = model_usage {
        normalize_credit_model_summary_list(model_usage)
    } else {
        normalize_model_summary_list(
            params
                .get("modelData")
                .and_then(Value::as_object)
                .unwrap_or(&serde_json::Map::new()),
        )
    };
    let tool_summary_list = if let Some(tool_usage) = tool_usage {
        normalize_credit_tool_summary_list(tool_usage)
    } else {
        normalize_tool_summary_list(
            params
                .get("toolData")
                .and_then(Value::as_object)
                .unwrap_or(&serde_json::Map::new()),
        )
    };
    let model_data_list = if let Some(model_usage) = model_usage {
        normalize_credit_model_data_list(model_usage, x_time, granularity)
    } else {
        normalize_model_data_list(
            params
                .get("modelData")
                .and_then(Value::as_object)
                .unwrap_or(&serde_json::Map::new()),
            x_time,
            granularity,
        )
    };
    let tool_data_list = if let Some(tool_usage) = tool_usage {
        normalize_credit_tool_data_list(tool_usage, x_time, granularity)
    } else {
        normalize_tool_data_list(
            params
                .get("toolData")
                .and_then(Value::as_object)
                .unwrap_or(&serde_json::Map::new()),
            x_time,
            granularity,
        )
    };
    let total_tokens_usage = if let Some(model_usage) = model_usage {
        sum_credit_model_tokens(model_usage)
    } else {
        params
            .get("modelData")
            .and_then(Value::as_object)
            .and_then(|data| object(data.get("totalUsage")))
            .and_then(|total| number_at(total, "totalTokensUsage"))
            .unwrap_or(0.0)
    };
    let total_model_call_count = if let Some(model_usage) = model_usage {
        array(model_usage.get("modelDataList")).len() as f64
    } else {
        params
            .get("modelData")
            .and_then(Value::as_object)
            .and_then(|data| object(data.get("totalUsage")))
            .and_then(|total| number_at(total, "totalModelCallCount"))
            .unwrap_or(0.0)
    };
    let mut activity = build_activity_snapshot(params.get("activityData"));
    activity["summary"]["favoriteModelName"] = model_summary_list
        .first()
        .map(|model| model["modelName"].clone())
        .unwrap_or(Value::Null);
    json!({
        "range": request.get("range").and_then(Value::as_str).unwrap_or("today"),
        "rangeStartDate": params.get("startDateKey").and_then(Value::as_str).unwrap_or(""),
        "rangeEndDate": params.get("endDateKey").and_then(Value::as_str).unwrap_or(""),
        "generatedAt": generated_at,
        "sourceProvider": provider,
        "quota": quota,
        "activity": activity,
        "detail": build_detail_snapshot(params.get("modelDetailData"), params.get("toolDetailData")),
        "modelUsage": {
            "xTime": x_time_value,
            "granularity": granularity,
            "totalModelCallCount": number(total_model_call_count),
            "totalTokensUsage": number(total_tokens_usage),
            "modelDataList": model_data_list,
            "modelSummaryList": model_summary_list,
        },
        "toolUsage": {
            "xTime": x_time_value,
            "granularity": granularity,
            "toolDataList": tool_data_list,
            "toolSummaryList": tool_summary_list,
        },
        "health": build_health_snapshot(params.get("healthData")),
    })
}
