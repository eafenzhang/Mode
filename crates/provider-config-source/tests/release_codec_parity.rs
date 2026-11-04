//! Built-in Release 编解码与 TS `mode-builtin-release.ts` 的对拍。
//!
//! fixture 由 `pnpm builtin-release:fixtures` 生成：`valid` 使用仓库内真实
//! Release（`config/provider/mode-builtin.json`），保存 TS 端 `encode` 结果与
//! `JSON.stringify` 原文；`rejects` 保存 TS 端会拒绝的最小文档。Rust 侧要求
//! 逐字节复现 TS 的规范序列化，并保持接受/拒绝对齐。

use std::path::PathBuf;

use provider_config::JsonValue;
use provider_config_source::{decode_mode_builtin_release, serialize_mode_builtin_release};

fn load_fixture() -> JsonValue {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("builtin-release.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 {} 失败: {error}", path.display()));
    JsonValue::from_json_str(&text).expect("fixture 必须是合法 JSON")
}

fn object<'a>(value: &'a JsonValue, key: &str) -> &'a JsonValue {
    value
        .as_object()
        .and_then(|map| map.get(key))
        .unwrap_or_else(|| panic!("fixture 缺少 {key}"))
}

#[test]
fn real_release_serializes_byte_for_byte_like_ts() {
    let fixture = load_fixture();
    let valid = object(&fixture, "valid");
    let release = decode_mode_builtin_release(object(valid, "input"))
        .unwrap_or_else(|error| panic!("真实 Release 解码失败: {error}"));

    let expected_revision = object(valid, "revision")
        .as_number()
        .and_then(|number| number.as_i64())
        .expect("valid.revision");
    assert_eq!(release.revision, expected_revision);

    let expected_serialized = object(valid, "serialized")
        .as_str()
        .expect("valid.serialized");
    assert_eq!(
        serialize_mode_builtin_release(&release),
        expected_serialized
    );

    // encode 结构对拍：键序由字符串比较覆盖，这里再确认 JSON 值等价。
    assert_eq!(
        release.to_json(),
        *object(valid, "encoded"),
        "encodeModeBuiltinRelease 结构不一致"
    );
}

#[test]
fn ts_rejected_documents_are_rejected_here() {
    let fixture = load_fixture();
    let rejects = object(&fixture, "rejects")
        .as_array()
        .expect("fixture.rejects");
    assert!(!rejects.is_empty(), "rejects 不能为空");

    for case in rejects {
        let name = object(case, "name").as_str().expect("case.name");
        let result = decode_mode_builtin_release(object(case, "input"));
        let error = result
            .err()
            .unwrap_or_else(|| panic!("{name}: TS 拒绝但 Rust 接受"));
        if name == "retired-zapi-provider" {
            // 该错误不是 zod 校验失败，TS 与 Rust 的文案一致。
            assert!(
                error.to_string().contains("已退出的 Provider"),
                "{name}: 错误文案不一致: {error}"
            );
        }
    }
}
