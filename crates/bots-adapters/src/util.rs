//! 解析共用小工具：TS 各 provider 的 `readString/readNumber/readNumberOrString` 口径。
//!
//! 注意 trim 口径按通道区分：飞书/企微/微信的 `readString` **不 trim**（由调用方按需
//! trim），钉钉的 `readString` **trim**（见 [`crate::dingtalk`] 内部实现）。

use serde_json::Value;

pub(crate) fn is_record(value: &Value) -> bool {
    value.is_object()
}

/// TS `readString`：仅字符串取值，不 trim。
pub(crate) fn read_str(record: &Value, key: &str) -> String {
    record
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// TS `readNumber`：仅有限数值。
pub(crate) fn read_number(record: &Value, key: &str) -> Option<f64> {
    record.get(key).and_then(Value::as_f64)
}

/// TS `readNumberOrString`：字符串原样；有限数值转十进制字符串。
pub(crate) fn read_number_or_string(record: &Value, key: &str) -> String {
    match record.get(key) {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Number(number)) => number
            .as_f64()
            .filter(|value| value.is_finite())
            .map(|value| {
                if value.fract() == 0.0 && value.abs() < 9_007_199_254_740_992.0 {
                    format!("{}", value as i64)
                } else {
                    value.to_string()
                }
            })
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// TS `parseJsonRecord`：对象直取；字符串尝试 JSON.parse 且结果必须是对象。
pub(crate) fn parse_json_record(value: Option<&Value>) -> Option<Value> {
    match value {
        Some(value) if value.is_object() => Some(value.clone()),
        Some(Value::String(text)) => serde_json::from_str::<Value>(text)
            .ok()
            .filter(Value::is_object),
        _ => None,
    }
}

/// 依次尝试取值，取第一个非空（TS 的 `a || b || c` 链）。
pub(crate) fn first_non_empty(candidates: impl IntoIterator<Item = String>) -> String {
    candidates
        .into_iter()
        .find(|value| !value.is_empty())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn number_or_string_matches_ts() {
        assert_eq!(read_number_or_string(&json!({"a":"x"}), "a"), "x");
        assert_eq!(read_number_or_string(&json!({"a":123}), "a"), "123");
        assert_eq!(read_number_or_string(&json!({"a":1.5}), "a"), "1.5");
        assert_eq!(read_number_or_string(&json!({"a":true}), "a"), "");
        assert_eq!(read_number_or_string(&json!({"a":"  x  "}), "a"), "  x  ");
    }

    #[test]
    fn parse_json_record_only_accepts_objects() {
        assert_eq!(
            parse_json_record(Some(&json!({"a":1}))).unwrap(),
            json!({"a":1})
        );
        assert_eq!(
            parse_json_record(Some(&json!("{\"a\":1}"))).unwrap(),
            json!({"a":1})
        );
        assert!(parse_json_record(Some(&json!("[1]"))).is_none());
        assert!(parse_json_record(Some(&json!("not-json"))).is_none());
        assert!(parse_json_record(None).is_none());
    }

    #[test]
    fn first_non_empty_picks_first_truthy() {
        assert_eq!(
            first_non_empty(["".to_string(), "b".to_string(), "c".to_string()]),
            "b"
        );
        assert_eq!(first_non_empty(Vec::<String>::new()), "");
    }
}
