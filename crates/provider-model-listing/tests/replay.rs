//! 与 TS「检测可用模型」请求构造/响应解析的对拍。

use std::collections::BTreeMap;
use std::path::Path;

use provider_model_listing::{
    build_remote_model_list_request, parse_remote_model_list_page, remote_model_list_page_to_json,
    remote_model_list_request_to_json, RemoteModelCatalogRequest,
};
use serde_json::Value;

fn read_fixture() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("model-listing.json");
    let text = std::fs::read_to_string(&path).expect("读取 model-listing.json");
    serde_json::from_str(&text).expect("解析 fixture")
}

fn request_from_json(value: &Value) -> RemoteModelCatalogRequest {
    RemoteModelCatalogRequest {
        api_type: value["apiType"].as_str().expect("apiType").to_string(),
        base_url: value["baseUrl"].as_str().expect("baseUrl").to_string(),
        api_key: value
            .get("apiKey")
            .and_then(Value::as_str)
            .map(str::to_string),
        headers: value
            .get("headers")
            .and_then(Value::as_object)
            .map(|object| {
                object
                    .iter()
                    .map(|(key, value)| {
                        (key.clone(), value.as_str().expect("header 值").to_string())
                    })
                    .collect::<BTreeMap<String, String>>()
            }),
    }
}

#[test]
fn matches_typescript_build_requests() {
    let fixture = read_fixture();
    for (index, case) in fixture["build"]
        .as_array()
        .expect("build")
        .iter()
        .enumerate()
    {
        let name = case["name"].as_str().expect("case name");
        let input = request_from_json(&case["input"]);
        match build_remote_model_list_request(&input) {
            Ok(request) => {
                assert!(
                    case.get("error").is_none(),
                    "build[{index}] {name}: 期望错误但 TS 用例成功"
                );
                assert_eq!(
                    remote_model_list_request_to_json(&request),
                    case["result"].clone(),
                    "build[{index}] {name}"
                );
            }
            Err(error) => {
                assert_eq!(
                    Value::String(error),
                    case["error"].clone(),
                    "build[{index}] {name}"
                );
            }
        }
    }
}

#[test]
fn matches_typescript_parse_pages() {
    let fixture = read_fixture();
    for (index, case) in fixture["parse"]
        .as_array()
        .expect("parse")
        .iter()
        .enumerate()
    {
        let name = case["name"].as_str().expect("case name");
        match parse_remote_model_list_page(&case["payload"]) {
            Ok(page) => {
                assert!(
                    case.get("error").is_none(),
                    "parse[{index}] {name}: 期望错误但 TS 用例成功"
                );
                assert_eq!(
                    remote_model_list_page_to_json(&page),
                    case["result"].clone(),
                    "parse[{index}] {name}"
                );
            }
            Err(error) => {
                assert_eq!(
                    Value::String(error),
                    case["error"].clone(),
                    "parse[{index}] {name}"
                );
            }
        }
    }
}
