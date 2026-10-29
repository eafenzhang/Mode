//! ECMAScript 兼容的小工具。
//!
//! 任务库的冻结语义里有 `String.prototype.trim`、`\s` 归一化与
//! `decodeURIComponent` 三处直接依赖 JS 运行时行为。Rust 的同名 API 集合不完全一致
//! （例如 Rust 的 `char::is_whitespace` 包含 `U+0085`、不含 `U+FEFF`），这里按
//! ECMAScript 的 WhiteSpace + LineTerminator 集合显式复刻，避免近似。

/// ECMAScript `WhiteSpace` + `LineTerminator`。
pub(crate) fn is_js_whitespace(value: char) -> bool {
    matches!(
        value,
        '\u{0009}'
            | '\u{000A}'
            | '\u{000B}'
            | '\u{000C}'
            | '\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
            | '\u{FEFF}'
    )
}

pub(crate) fn js_trim(value: &str) -> &str {
    value.trim_matches(is_js_whitespace)
}

/// 等价于 JS `String(sql ?? "").replace(/\s+/g, " ").trim()`。
pub(crate) fn normalize_schema_sql(sql: &str) -> String {
    let mut result = String::with_capacity(sql.len());
    let mut pending_space = false;
    for value in sql.chars() {
        if is_js_whitespace(value) {
            pending_space = !result.is_empty();
            continue;
        }
        if pending_space {
            result.push(' ');
            pending_space = false;
        }
        result.push(value);
    }
    result
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

/// 等价于 JS `decodeURIComponent`，失败时返回 `None`（TS 侧回退原文）。
pub(crate) fn decode_component(value: &str) -> String {
    decode_uri_component(value).unwrap_or_else(|| value.to_owned())
}

fn decode_uri_component(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = hex_digit(*bytes.get(index + 1)?)?;
            let low = hex_digit(*bytes.get(index + 2)?)?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    // `decodeURIComponent` 对非法 UTF-8（以及孤立代理项、超长编码）抛 URIError。
    String::from_utf8(decoded).ok()
}

#[cfg(test)]
mod tests {
    use super::{decode_component, js_trim, normalize_schema_sql};

    #[test]
    fn decodes_percent_sequences_and_falls_back() {
        assert_eq!(decode_component("model%2Fone"), "model/one");
        assert_eq!(decode_component("m%C3%A9"), "mé");
        assert_eq!(decode_component("plain"), "plain");
        assert_eq!(decode_component("bad%ZZ"), "bad%ZZ");
        assert_eq!(decode_component("bad%"), "bad%");
        assert_eq!(decode_component("%C3"), "%C3");
    }

    #[test]
    fn trims_with_ecmascript_whitespace_set() {
        assert_eq!(js_trim("\u{FEFF} value \u{00A0}"), "value");
        // Rust 的 char::is_whitespace 会裁掉 NEL，JS 不会。
        assert_eq!(js_trim("\u{0085}value"), "\u{0085}value");
    }

    #[test]
    fn normalizes_sql_whitespace_like_js() {
        assert_eq!(
            normalize_schema_sql("\n  CREATE INDEX x\n   ON y (a)\n"),
            "CREATE INDEX x ON y (a)"
        );
        assert_eq!(normalize_schema_sql(""), "");
    }
}
