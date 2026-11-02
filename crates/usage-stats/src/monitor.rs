//! BigModel monitor（legacy model-usage / tool-usage）→ UsageStatsSnapshot 投影。
//!
//! 真源：`packages/services/src/usage-stats/providers/bigmodelUsageMonitorMapper.ts`
//! 的 `buildUsageStatsSnapshotFromMonitor` 与 `buildToolUsages`。
//! 时间由调用方注入，不读系统时钟。

use serde_json::{json, Number, Value};

const ESTIMATED_TOKEN_CHAR_DIVISOR: i64 = 3;

fn number(value: f64) -> Value {
    if value.is_finite() && value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_992.0 {
        Value::Number(Number::from(value as i64))
    } else {
        Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

fn number_field(object: &serde_json::Map<String, Value>, key: &str) -> Option<f64> {
    object.get(key).and_then(Value::as_f64)
}

fn array_field<'a>(object: &'a serde_json::Map<String, Value>, key: &str) -> &'a [Value] {
    object
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn value_at(values: &[Value], index: usize) -> f64 {
    values.get(index).and_then(Value::as_f64).unwrap_or(0.0)
}

fn sum_values(values: &[Value]) -> f64 {
    values.iter().filter_map(Value::as_f64).sum()
}

fn day_summary(date: &str, tokens: f64, calls: f64) -> Value {
    json!({
        "date": date,
        "label": date,
        "totalCharacters": number(tokens * ESTIMATED_TOKEN_CHAR_DIVISOR as f64),
        "totalEstimatedTokens": number(tokens),
        "sessionCount": if calls > 0.0 { 1 } else { 0 },
        "messageCount": number(calls),
        "activityScore": number(tokens),
    })
}

fn build_tool_usages(tool_data: &serde_json::Map<String, Value>) -> Vec<Value> {
    let total_usage = tool_data.get("totalUsage").and_then(Value::as_object);
    let definitions = [
        (
            "network-search",
            "联网搜索 MCP",
            "networkSearchCount",
            "totalNetworkSearchCount",
        ),
        (
            "web-reader",
            "网页读取 MCP",
            "webReadMcpCount",
            "totalWebReadMcpCount",
        ),
        (
            "zread",
            "开源仓库 MCP",
            "zreadMcpCount",
            "totalZreadMcpCount",
        ),
    ];
    let mut tools = Vec::new();
    for (tool_code, display_name, daily_key, total_key) in definitions {
        let daily = array_field(tool_data, daily_key);
        let total = total_usage
            .and_then(|total| number_field(total, total_key))
            .unwrap_or_else(|| sum_values(daily));
        tools.push(json!({
            "toolCode": tool_code,
            "displayName": display_name,
            "totalCalls": number(total),
            "dailyCalls": daily.iter().map(|value| number(value.as_f64().unwrap_or(0.0))).collect::<Vec<_>>(),
        }));
    }
    let total_search_mcp = total_usage
        .and_then(|total| number_field(total, "totalSearchMcpCount"))
        .unwrap_or(0.0);
    if total_search_mcp > 0.0 {
        tools.push(json!({
            "toolCode": "search-mcp",
            "displayName": "Search MCP",
            "totalCalls": number(total_search_mcp),
            "dailyCalls": [],
        }));
    }
    tools
}

pub fn build_usage_stats_snapshot_from_monitor(
    request: &Value,
    model_data: &Value,
    tool_data: &Value,
    generated_at: i64,
) -> Value {
    let request = request.as_object().cloned().unwrap_or_default();
    let model_data = model_data.as_object().cloned().unwrap_or_default();
    let tool_data = tool_data.as_object().cloned().unwrap_or_default();
    let x_time = array_field(&model_data, "x_time");
    let tokens_usage = array_field(&model_data, "tokensUsage");
    let model_call_count = array_field(&model_data, "modelCallCount");
    let daily: Vec<Value> = x_time
        .iter()
        .enumerate()
        .map(|(index, date)| {
            day_summary(
                date.as_str().unwrap_or(""),
                value_at(tokens_usage, index),
                value_at(model_call_count, index),
            )
        })
        .collect();
    let total_usage = model_data.get("totalUsage").and_then(Value::as_object);
    let total_tokens = total_usage
        .and_then(|total| number_field(total, "totalTokensUsage"))
        .unwrap_or(0.0);
    let total_calls = total_usage
        .and_then(|total| number_field(total, "totalModelCallCount"))
        .unwrap_or(0.0);
    let active_days = daily
        .iter()
        .filter(|day| day["totalEstimatedTokens"].as_f64().unwrap_or(0.0) > 0.0)
        .count();
    let most_active_day = daily
        .iter()
        .fold(None::<&Value>, |best, day| match best {
            None => Some(day),
            Some(best)
                if day["totalEstimatedTokens"].as_f64().unwrap_or(0.0)
                    > best["totalEstimatedTokens"].as_f64().unwrap_or(0.0) =>
            {
                Some(day)
            }
            Some(best) => Some(best),
        })
        .cloned();
    let first_active_day = daily
        .iter()
        .find(|day| day["totalEstimatedTokens"].as_f64().unwrap_or(0.0) > 0.0)
        .cloned();
    let last_active_day = daily
        .iter()
        .rev()
        .find(|day| day["totalEstimatedTokens"].as_f64().unwrap_or(0.0) > 0.0)
        .cloned();
    let summary_list = total_usage
        .and_then(|total| total.get("modelSummaryList"))
        .filter(|value| !value.is_null())
        .and_then(Value::as_array)
        .or_else(|| model_data.get("modelSummaryList").and_then(Value::as_array))
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut models: Vec<Value> = summary_list
        .iter()
        .filter_map(|entry| {
            let entry = entry.as_object()?;
            let model_name = entry.get("modelName")?.as_str()?;
            if model_name.is_empty() {
                return None;
            }
            let total = number_field(entry, "totalTokens").unwrap_or(0.0);
            Some(json!({
                "modelId": model_name,
                "totalCharacters": number(total * ESTIMATED_TOKEN_CHAR_DIVISOR as f64),
                "totalEstimatedTokens": number(total),
                "inputCharacters": 0,
                "inputEstimatedTokens": 0,
                "outputCharacters": 0,
                "outputEstimatedTokens": 0,
                "sessionCount": 0,
                "messageCount": 0,
                "share": number(if total_tokens > 0.0 { total / total_tokens } else { 0.0 }),
            }))
        })
        .collect();
    models.sort_by(|left, right| {
        right["totalEstimatedTokens"]
            .as_f64()
            .unwrap_or(0.0)
            .total_cmp(&left["totalEstimatedTokens"].as_f64().unwrap_or(0.0))
    });
    let favorite_model = models.first().map(|model| {
        json!({
            "modelId": model["modelId"],
            "totalCharacters": model["totalCharacters"],
            "totalEstimatedTokens": model["totalEstimatedTokens"],
            "share": model["share"],
        })
    });
    let max_activity = daily
        .iter()
        .map(|day| day["activityScore"].as_f64().unwrap_or(0.0))
        .fold(0.0_f64, f64::max);
    json!({
        "range": request.get("range").and_then(Value::as_str).unwrap_or("all"),
        "generatedAt": generated_at,
        "timeZone": request.get("timeZone").and_then(Value::as_str).unwrap_or("UTC"),
        "estimatedTokenCharDivisor": ESTIMATED_TOKEN_CHAR_DIVISOR,
        "summary": {
            "totalSessions": number(total_calls),
            "totalCharacters": number(total_tokens * ESTIMATED_TOKEN_CHAR_DIVISOR as f64),
            "totalEstimatedTokens": number(total_tokens),
            "totalMessages": number(total_calls),
            "activeDays": active_days,
            "mostActiveDay": most_active_day,
            "favoriteModel": favorite_model,
            "longestSessionMs": 0,
            "longestStreakDays": 0,
            "currentStreakDays": 0,
            "firstActivityDate": first_active_day.as_ref().map(|day| day["date"].clone()).unwrap_or(Value::Null),
            "lastActivityDate": last_active_day.as_ref().map(|day| day["date"].clone()).unwrap_or(Value::Null),
            "peakHour": null,
        },
        "daily": daily,
        "heatmap": {
            "startDate": first_active_day.as_ref().map(|day| day["date"].clone()).unwrap_or(Value::Null),
            "endDate": last_active_day.as_ref().map(|day| day["date"].clone()).unwrap_or(Value::Null),
            "maxActivityScore": number(max_activity),
            "weeks": [],
            "monthLabels": [],
        },
        "models": models,
        "source": "bigmodel-monitor",
        "tools": build_tool_usages(&tool_data),
    })
}
