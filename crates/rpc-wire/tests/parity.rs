//! 与 TS 实现的黄金字节逐字节对拍（`scripts/generate-rpc-wire-fixtures.ts` 产出）。

use rpc_wire::{deserialize, serialize};

#[derive(serde::Deserialize)]
struct Fixture {
    entries: Vec<Entry>,
}

#[derive(serde::Deserialize)]
struct Entry {
    name: String,
    hex: String,
}

fn load() -> Fixture {
    let raw = include_str!("fixtures/values.json");
    serde_json::from_str(raw).expect("fixtures/values.json 必须是合法 JSON")
}

fn from_hex(hex: &str) -> Vec<u8> {
    assert_eq!(hex.len() % 2, 0, "十六进制长度必须是偶数");
    (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).expect("十六进制字符"))
        .collect()
}

#[test]
fn decodes_golden_bytes_and_reencodes_identically() {
    let fixture = load();
    assert!(!fixture.entries.is_empty(), "黄金文件不应为空");
    for entry in &fixture.entries {
        let expected = from_hex(&entry.hex);
        let value =
            deserialize(&expected).unwrap_or_else(|error| panic!("{} 解码失败: {error:?}", entry.name));
        let actual = serialize(&value);
        assert_eq!(
            actual, expected,
            "{} 再编码后与 TS 字节不一致：\n  TS  = {}\n  Rust= {}",
            entry.name,
            entry.hex,
            actual.iter().map(|byte| format!("{byte:02x}")).collect::<String>()
        );
    }
}

#[test]
fn golden_file_covers_every_tag() {
    let fixture = load();
    let decoded: Vec<(String, Vec<u8>)> = fixture
        .entries
        .iter()
        .map(|entry| (entry.name.clone(), from_hex(&entry.hex)))
        .collect();
    for (tag, label) in [
        (0u8, "undefined"),
        (1, "string"),
        (2, "uint8array"),
        (3, "vsbuffer"),
        (4, "array"),
        (5, "json object"),
        (6, "int"),
    ] {
        assert!(
            decoded.iter().any(|(_, bytes)| bytes.first() == Some(&tag)),
            "黄金文件缺少 tag {tag}（{label}）的样例"
        );
    }
}
