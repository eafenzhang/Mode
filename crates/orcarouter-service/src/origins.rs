//! OrcaRouter origins 与授权/目录 URL 构造（TS `packages/shared/src/orcarouter.ts` 子集）。

use reqwest::Url;

pub const ORCAROUTER_DEFAULT_AUTH_BASE: &str = "https://www.orcarouter.ai";
pub const ORCAROUTER_DEFAULT_API_BASE: &str = "https://api.orcarouter.ai";
pub const ORCAROUTER_AUTHORIZE_PATH: &str = "/auth";
pub const ORCAROUTER_EXCHANGE_PATH: &str = "/api/v1/auth/keys";
pub const ORCAROUTER_MODELS_PATH: &str = "/v1/models";
pub const ORCAROUTER_CODE_CHALLENGE_METHOD: &str = "S256";
pub const ORCAROUTER_REQUESTED_SCOPE: &str = "api";

pub const ORCAROUTER_BASE_URL_ENV: &str = "ORCA_BASE_URL";
pub const ORCAROUTER_AUTH_BASE_URL_ENV: &str = "ORCA_AUTH_BASE_URL";
pub const ORCAROUTER_API_BASE_URL_ENV: &str = "ORCA_API_BASE_URL";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrcaOrigins {
    pub auth_base: String,
    pub api_base: String,
}

fn strip_trailing_slash(value: &str) -> String {
    value.trim_end_matches('/').to_string()
}

fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
    matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1")
}

/// 远端 origin 强制 HTTPS；HTTP 只允许 loopback。
pub fn normalize_orca_origin(value: &str, label: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{label} 不能为空"));
    }
    let mut url = Url::parse(trimmed).map_err(|_| format!("{label} 不是合法 URL"))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(format!("{label} 不允许携带用户信息"));
    }
    match url.scheme() {
        "http" => {
            let host = url.host_str().unwrap_or_default();
            if !is_loopback_host(host) {
                return Err(format!(
                    "{label} 只允许 loopback 使用 http，远端必须使用 https"
                ));
            }
        }
        "https" => {}
        _ => return Err(format!("{label} 只支持 http(s)")),
    }
    url.set_query(None);
    url.set_fragment(None);
    Ok(strip_trailing_slash(url.as_str()))
}

/// 优先级：显式 AUTH/API > ORCA_BASE_URL > 官方默认；逐字段回退，不互相推导。
pub fn resolve_orca_origins(env: impl Fn(&str) -> Option<String>) -> Result<OrcaOrigins, String> {
    let shared = env(ORCAROUTER_BASE_URL_ENV)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let explicit_auth = env(ORCAROUTER_AUTH_BASE_URL_ENV)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let explicit_api = env(ORCAROUTER_API_BASE_URL_ENV)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let auth_base = explicit_auth
        .or_else(|| shared.clone())
        .unwrap_or_else(|| ORCAROUTER_DEFAULT_AUTH_BASE.to_string());
    let api_base = explicit_api
        .or(shared)
        .unwrap_or_else(|| ORCAROUTER_DEFAULT_API_BASE.to_string());
    Ok(OrcaOrigins {
        auth_base: normalize_orca_origin(&auth_base, "OrcaRouter 授权地址")?,
        api_base: normalize_orca_origin(&api_base, "OrcaRouter 推理地址")?,
    })
}

/// 由 api origin 构造 `/v1` 推理基址。
pub fn build_orca_v1_base(api_base: &str) -> String {
    format!("{}/v1", strip_trailing_slash(api_base))
}

/// 由 api origin 构造模型目录 URL（绝不复用 auth origin）。
pub fn build_orca_models_url(api_base: &str) -> String {
    format!(
        "{}{}",
        strip_trailing_slash(api_base),
        ORCAROUTER_MODELS_PATH
    )
}

pub struct OrcaAuthorizeUrlInput<'a> {
    pub auth_base: &'a str,
    pub callback_url: &'a str,
    pub code_challenge: &'a str,
    pub state: &'a str,
    pub app_name: &'a str,
    pub scope: Option<&'a str>,
}

/// OOB + S256 授权 URL（参数插入序与 TS 一致）。
pub fn build_orca_authorize_url(input: OrcaAuthorizeUrlInput<'_>) -> Result<String, String> {
    let mut url = Url::parse(&format!(
        "{}{}",
        strip_trailing_slash(input.auth_base),
        ORCAROUTER_AUTHORIZE_PATH
    ))
    .map_err(|error| error.to_string())?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("callback_url", input.callback_url);
        query.append_pair("code_challenge", input.code_challenge);
        query.append_pair("code_challenge_method", ORCAROUTER_CODE_CHALLENGE_METHOD);
        query.append_pair("state", input.state);
        query.append_pair("app_name", input.app_name);
        query.append_pair("scope", input.scope.unwrap_or(ORCAROUTER_REQUESTED_SCOPE));
    }
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: std::collections::HashMap<String, String> = pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        move |key| map.get(key).cloned()
    }

    #[test]
    fn normalize_enforces_https_except_loopback() {
        assert_eq!(
            normalize_orca_origin("https://Example.com/", "auth").expect("https"),
            "https://example.com"
        );
        assert!(normalize_orca_origin("http://example.com", "auth").is_err());
        assert_eq!(
            normalize_orca_origin("http://127.0.0.1:8080/", "auth").expect("loopback"),
            "http://127.0.0.1:8080"
        );
        assert!(normalize_orca_origin("https://u:p@example.com", "auth").is_err());
        assert!(normalize_orca_origin("ftp://example.com", "auth").is_err());
    }

    #[test]
    fn resolve_origins_priority() {
        let origins = resolve_orca_origins(env(&[])).expect("defaults");
        assert_eq!(origins.auth_base, ORCAROUTER_DEFAULT_AUTH_BASE);
        assert_eq!(origins.api_base, ORCAROUTER_DEFAULT_API_BASE);

        let origins = resolve_orca_origins(env(&[
            (ORCAROUTER_BASE_URL_ENV, "https://shared.example"),
            (ORCAROUTER_AUTH_BASE_URL_ENV, "https://auth.example"),
        ]))
        .expect("mixed");
        assert_eq!(origins.auth_base, "https://auth.example");
        assert_eq!(origins.api_base, "https://shared.example");
    }

    #[test]
    fn authorize_url_uses_oob_and_s256() {
        let url = build_orca_authorize_url(OrcaAuthorizeUrlInput {
            auth_base: "https://www.orcarouter.ai",
            callback_url: "oob",
            code_challenge: "challenge",
            state: "state-1",
            app_name: "Mode",
            scope: None,
        })
        .expect("url");
        assert!(url.starts_with("https://www.orcarouter.ai/auth?"));
        assert!(url.contains("callback_url=oob"));
        assert!(url.contains("code_challenge=challenge"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("scope=api"));
    }
}
