//! Provider「检测可用模型」的请求构造与响应分页解析（纯逻辑）。
//!
//! 真源：`packages/provider/src/model-listing.ts`。
//! 只构造请求与解析响应，不发网络；HTTP 执行由调用方注入。

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

/** Anthropic 兼容网关同时读取 x-api-key 和 Bearer Authorization；版本号与执行链共用同一档。 */
const ANTHROPIC_VERSION_HEADER_VALUE: &str = "2023-06-01";
const ANTHROPIC_PAGE_LIMIT: &str = "1000";

/// 设置页「检测可用模型」的草稿连接信息；全部来自表单草稿，不要求先保存。
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteModelCatalogRequest {
    /// TS 侧是 `ProviderApiType` 联合类型；这里按运行时字符串处理，
    /// 非法值与 TS 的 `default` 分支一样显式报错。
    pub api_type: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub headers: Option<BTreeMap<String, String>>,
}

/// 一次可执行的模型列表 HTTP 请求。URL/头语义必须与正式模型执行链一致。
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteModelListRequest {
    pub url: String,
    pub headers: BTreeMap<String, String>,
}

/// 单页解析结果；cursor 存在表示还有下一页（Anthropic `after` 游标）。
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteModelListPage {
    pub model_ids: Vec<String>,
    pub cursor: Option<String>,
}

pub fn remote_model_list_request_to_json(request: &RemoteModelListRequest) -> Value {
    let headers: Map<String, Value> = request
        .headers
        .iter()
        .map(|(key, value)| (key.clone(), json!(value)))
        .collect();
    json!({ "url": request.url, "headers": headers })
}

pub fn remote_model_list_page_to_json(page: &RemoteModelListPage) -> Value {
    let mut object = Map::new();
    object.insert("modelIds".to_string(), json!(page.model_ids));
    if let Some(cursor) = &page.cursor {
        object.insert("cursor".to_string(), json!(cursor));
    }
    Value::Object(object)
}

/// Anthropic 的 SDK 会在 baseURL 后追加 /v1/messages，网关根必须先补齐 /v1 前缀。
fn normalize_anthropic_base_pathname(pathname: &str) -> String {
    let trimmed = pathname.trim_end_matches('/');
    if trimmed.to_lowercase().ends_with("/v1") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1")
    }
}

/// 显式配置的 Provider headers 拥有最高优先级；检测头不覆盖同名（大小写不敏感）显式头。
fn merge_with_explicit_priority(
    generated: BTreeMap<String, String>,
    explicit: Option<&BTreeMap<String, String>>,
) -> BTreeMap<String, String> {
    let Some(explicit) = explicit else {
        return generated;
    };
    let explicit_names: std::collections::BTreeSet<String> =
        explicit.keys().map(|key| key.to_lowercase()).collect();
    let mut merged: BTreeMap<String, String> = generated
        .into_iter()
        .filter(|(key, _)| !explicit_names.contains(&key.to_lowercase()))
        .collect();
    for (key, value) in explicit {
        merged.insert(key.clone(), value.clone());
    }
    merged
}

/// 按 Provider API 类型构造模型列表请求。
/// openai 系 baseURL 原样使用（不插入 /v1）；anthropic 系复用执行链的 /v1 归一化规则，
/// 否则会出现「检测通过但正式执行失败」的语义分叉。
pub fn build_remote_model_list_request(
    input: &RemoteModelCatalogRequest,
) -> Result<RemoteModelListRequest, String> {
    let base_url = input.base_url.trim();
    if base_url.is_empty() {
        return Err("Base URL 不能为空".to_string());
    }
    let mut url = url::Url::parse(base_url).map_err(|_| "Invalid URL".to_string())?;
    if url.scheme() != "https" && url.scheme() != "http" {
        // TS 的 `url.protocol` 带冒号，错误文案保持同一口径。
        return Err(format!("Base URL 协议不受支持: {}:", url.scheme()));
    }
    let api_key = input.api_key.as_deref().map(str::trim).unwrap_or("");

    match input.api_type.as_str() {
        "openai-chat-completions" | "openai-responses" => {
            let trimmed = url.path().trim_end_matches('/');
            url.set_path(&format!("{trimmed}/models"));
            let mut generated = BTreeMap::new();
            if !api_key.is_empty() {
                generated.insert("Authorization".to_string(), format!("Bearer {api_key}"));
            }
            Ok(RemoteModelListRequest {
                url: url.to_string(),
                headers: merge_with_explicit_priority(generated, input.headers.as_ref()),
            })
        }
        "anthropic-messages" => {
            let normalized = normalize_anthropic_base_pathname(url.path());
            url.set_path(&format!("{normalized}/models"));
            set_search_param(&mut url, "limit", ANTHROPIC_PAGE_LIMIT);
            let mut generated = BTreeMap::new();
            if !api_key.is_empty() {
                generated.insert("x-api-key".to_string(), api_key.to_string());
            }
            generated.insert(
                "anthropic-version".to_string(),
                ANTHROPIC_VERSION_HEADER_VALUE.to_string(),
            );
            if !api_key.is_empty() {
                generated.insert("Authorization".to_string(), format!("Bearer {api_key}"));
            }
            Ok(RemoteModelListRequest {
                url: url.to_string(),
                headers: merge_with_explicit_priority(generated, input.headers.as_ref()),
            })
        }
        other => Err(format!("不支持的 Provider API 类型: {other}")),
    }
}

/// JS `searchParams.set(name, value)` 语义：保留其它既有参数，同名（含已有 limit）替换。
/// `url` crate 的 `query_pairs_mut` 是在既有 query 上追加，先清空再按全部键值对重写。
fn set_search_param(url: &mut url::Url, name: &str, value: &str) {
    let existing: Vec<(String, String)> = url.query_pairs().into_owned().collect();
    let mut pairs: Vec<(String, String)> = existing
        .into_iter()
        .filter(|(key, _)| key != name)
        .collect();
    pairs.push((name.to_string(), value.to_string()));
    url.set_query(None);
    url.query_pairs_mut()
        .extend_pairs(pairs.iter().map(|(key, value)| (key, value)));
}

/// 把模型列表响应解析为一页结果。只接受 `{ data: [...] }` 标准形状（OpenAI / Anthropic 一致），
/// 缺失或非字符串 id 直接忽略；无法识别时显式报错，不做第二重猜测。
pub fn parse_remote_model_list_page(payload: &Value) -> Result<RemoteModelListPage, String> {
    let Some(object) = payload.as_object() else {
        return Err("Provider 模型列表响应格式无法识别".to_string());
    };
    let Some(data) = object.get("data").and_then(Value::as_array) else {
        return Err("Provider 模型列表响应缺少 data 数组".to_string());
    };
    let model_ids = data
        .iter()
        .filter_map(|item| {
            item.as_object()
                .and_then(|item| item.get("id"))
                .and_then(Value::as_str)
        })
        .filter(|id| !id.trim().is_empty())
        .map(|id| id.trim().to_string())
        .collect();
    let has_more = object.get("has_more") == Some(&Value::Bool(true));
    let last_id = object.get("last_id").and_then(Value::as_str);
    let cursor = if has_more {
        last_id.filter(|last| !last.is_empty()).map(str::to_string)
    } else {
        None
    };
    Ok(RemoteModelListPage { model_ids, cursor })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(
        api_type: &str,
        base_url: &str,
        api_key: Option<&str>,
        headers: Option<BTreeMap<String, String>>,
    ) -> RemoteModelCatalogRequest {
        RemoteModelCatalogRequest {
            api_type: api_type.to_string(),
            base_url: base_url.to_string(),
            api_key: api_key.map(str::to_string),
            headers,
        }
    }

    #[test]
    fn openai_keeps_base_url_and_merges_headers() {
        let mut explicit = BTreeMap::new();
        explicit.insert("authorization".to_string(), "draft".to_string());
        let built = build_remote_model_list_request(&request(
            "openai-chat-completions",
            "https://api.example.com/v1/",
            Some("sk-test"),
            Some(explicit),
        ))
        .expect("openai build");
        assert_eq!(built.url, "https://api.example.com/v1/models");
        assert_eq!(built.headers.get("Authorization"), None);
        assert_eq!(
            built.headers.get("authorization"),
            Some(&"draft".to_string())
        );
    }

    #[test]
    fn anthropic_appends_v1_and_page_limit() {
        let built = build_remote_model_list_request(&request(
            "anthropic-messages",
            "https://gateway.example.com",
            Some("sk-ant"),
            None,
        ))
        .expect("anthropic build");
        assert_eq!(
            built.url,
            "https://gateway.example.com/v1/models?limit=1000"
        );
        assert_eq!(built.headers.get("x-api-key"), Some(&"sk-ant".to_string()));
        assert_eq!(
            built.headers.get("anthropic-version"),
            Some(&"2023-06-01".to_string())
        );
        assert_eq!(
            built.headers.get("Authorization"),
            Some(&"Bearer sk-ant".to_string())
        );
    }

    #[test]
    fn anthropic_preserves_existing_query_and_replaces_limit() {
        let built = build_remote_model_list_request(&request(
            "anthropic-messages",
            "https://gateway.example.com/v1?team=a&limit=5",
            None,
            None,
        ))
        .expect("anthropic build");
        assert_eq!(
            built.url,
            "https://gateway.example.com/v1/models?team=a&limit=1000"
        );
    }

    #[test]
    fn rejects_empty_base_url_and_unsupported_scheme() {
        let error =
            build_remote_model_list_request(&request("openai-responses", "   ", None, None))
                .expect_err("empty base url");
        assert_eq!(error, "Base URL 不能为空");
        let error =
            build_remote_model_list_request(&request("openai-responses", "ftp://x", None, None))
                .expect_err("unsupported scheme");
        assert_eq!(error, "Base URL 协议不受支持: ftp:");
    }

    #[test]
    fn parses_page_with_cursor_semantics() {
        let page = parse_remote_model_list_page(&json!({
            "data": [{ "id": "a" }, { "id": " b " }, { "id": 123 }, {}, { "id": null }],
            "has_more": true,
            "last_id": "cursor-1",
        }))
        .expect("parse page");
        assert_eq!(page.model_ids, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(page.cursor.as_deref(), Some("cursor-1"));
        let empty_last = parse_remote_model_list_page(&json!({
            "data": [{ "id": "a" }],
            "has_more": true,
            "last_id": "",
        }))
        .expect("parse page");
        assert_eq!(empty_last.cursor, None);
    }

    #[test]
    fn rejects_unrecognized_payload_shapes() {
        for payload in [json!(null), json!([1]), json!("x"), json!({ "nope": true })] {
            assert!(parse_remote_model_list_page(&payload).is_err());
        }
    }
}
