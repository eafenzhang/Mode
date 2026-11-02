//! 与 TS `provider-config-file-codec.ts` 的对拍。

use std::path::Path;

use provider_config::JsonValue;
use provider_config_file_codec::{
    decode_provider_config_file, encode_provider_config_file, ProviderConfigCodecError,
};
use serde_json::Value;

#[test]
fn matches_typescript_provider_config_file_codec() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("files.json");
    let text = std::fs::read_to_string(&path).expect("读取 files.json");
    let fixture: Value = serde_json::from_str(&text).expect("解析 fixture");
    for (index, case) in fixture["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .enumerate()
    {
        let input = JsonValue::from_json_str(&case["input"].to_string()).expect("input JSON");
        let expected = &case["expected"];
        match decode_provider_config_file(&input) {
            Ok(update) => {
                assert_eq!(
                    expected["ok"].as_bool(),
                    Some(true),
                    "cases[{index}] 期望失败"
                );
                let encoded = serde_json::from_str::<Value>(
                    &encode_provider_config_file(&update).to_json_string(),
                )
                .expect("encoded JSON");
                assert_eq!(encoded, expected["encoded"], "cases[{index}] encoded");
            }
            Err(error) => {
                assert_eq!(
                    expected["ok"].as_bool(),
                    Some(false),
                    "cases[{index}] 期望成功"
                );
                match expected["kind"].as_str() {
                    Some("version") => match error {
                        ProviderConfigCodecError::UnsupportedVersion { message, version } => {
                            assert_eq!(message, expected["message"].as_str().unwrap_or(""));
                            assert_eq!(
                                version,
                                expected["version"].as_i64(),
                                "cases[{index}] version"
                            );
                        }
                        other => panic!("cases[{index}] 期望 version 错误: {other:?}"),
                    },
                    Some("invalid") => assert!(
                        matches!(error, ProviderConfigCodecError::Invalid(_)),
                        "cases[{index}] 期望 invalid: {error:?}"
                    ),
                    other => panic!("未知 expected.kind: {other:?}"),
                }
            }
        }
    }
}
