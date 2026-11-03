//! CUA broker 行协议装帧、方法注册表与 socket 路径铸造/发现（纯逻辑）。
//!
//! 真源：`packages/mode-cua/broker.js`（line codec、方法表、dispatch、
//! `mintBrokerSocketPath`/`resolveBrokerSocketPath`）。socket 传输
//! （`net.connect` + 超时与连接生命周期）与 helper host 进程归宿主装配；
//! 本模块只承载可对拍的装帧与判定。随机性与平台由调用方注入。

use serde_json::{json, Map, Value};

pub const BROKER_SOCKET_ENV: &str = "MODE_CUA_PERMISSION_BROKER_SOCKET";
pub const BROKER_UNAVAILABLE_ENV: &str = "MODE_CUA_PERMISSION_BROKER_UNAVAILABLE";

/// `BROKER_METHODS`（注入 rd_media 面与 raw_input 同表）。
pub const BROKER_METHODS: [&str; 12] = [
    "health",
    "list_apps",
    "list_windows",
    "observe",
    "capture",
    "perform",
    "raw_input",
    "launch_app",
    "screen_probe",
    "rd_media_start",
    "rd_media_switch",
    "rd_media_stop",
];

/// `READONLY_METHODS`：raw_input / perform / rd_media_* / launch_app 是写路径，
/// 故意不在只读表内（只读面不得触达注入）。
pub const READONLY_METHODS: [&str; 6] = [
    "health",
    "list_apps",
    "list_windows",
    "observe",
    "capture",
    "screen_probe",
];

pub fn is_broker_method(method: &str) -> bool {
    BROKER_METHODS.contains(&method)
}

pub fn is_read_only_broker_method(method: &str) -> bool {
    READONLY_METHODS.contains(&method)
}

/// 行协议请求（单请求单响应，一行一条）。
#[derive(Clone, Debug, PartialEq)]
pub struct BrokerRequest {
    pub id: Option<String>,
    pub method: String,
    pub params: Value,
}

/// 解析一行请求：非对象 / `method` 非字符串 / JSON 非法 → None。
/// `params` 缺失或 null 按 `{}`；`id` 非字符串按 null。
pub fn parse_request_line(line: &str) -> Option<BrokerRequest> {
    let parsed = serde_json::from_str::<Value>(line).ok()?;
    let object = parsed.as_object()?;
    let method = object.get("method")?.as_str()?.to_string();
    let params = match object.get("params") {
        Some(value) if !value.is_null() => value.clone(),
        _ => Value::Object(Map::new()),
    };
    let id = object.get("id").and_then(Value::as_str).map(str::to_string);
    Some(BrokerRequest { id, method, params })
}

pub fn ok_response(result: Value) -> Value {
    json!({ "ok": true, "result": result })
}

pub fn error_response(message: &str, code: &str) -> Value {
    json!({ "ok": false, "error": { "code": code, "message": message } })
}

/// TS `errorResponseFromException`：异常带字符串 `code` 时保留，否则 `internal`。
pub fn error_response_from_failure(message: &str, code: Option<&str>) -> Value {
    let code = code.filter(|value| !value.is_empty()).unwrap_or("internal");
    error_response(message, code)
}

/// 不含行尾换行：由写入方补 `\n`（单请求单响应，一行一条）。
pub fn serialize_response(response: &Value) -> String {
    serde_json::to_string(response).expect("response 序列化")
}

/// backend 注入口的返回形态：成功值，或带可选 `code` 的失败（TS 异常的
/// `message`/`code`）。
pub enum BackendOutcome {
    Ok(Value),
    Err {
        message: String,
        code: Option<String>,
    },
}

/// 分派：方法表外的先拒绝（`method_not_found`），backend 抛错收敛为
/// `errorResponseFromException` 语义，不做第二重猜测。
pub fn dispatch_request(
    backend: &mut dyn FnMut(&str, &Value) -> BackendOutcome,
    request: &BrokerRequest,
) -> Value {
    if !is_broker_method(&request.method) {
        return error_response(
            &format!("unknown method: {}", request.method),
            "method_not_found",
        );
    }
    match backend(&request.method, &request.params) {
        BackendOutcome::Ok(result) => ok_response(result),
        BackendOutcome::Err { message, code } => {
            error_response_from_failure(&message, code.as_deref())
        }
    }
}

/// 一行请求 → 一行响应（`handleRequestLine`）：装帧非法按 `invalid_request`。
pub fn handle_request_line(
    backend: &mut dyn FnMut(&str, &Value) -> BackendOutcome,
    line: &str,
) -> Value {
    let Some(request) = parse_request_line(line) else {
        return error_response("malformed request", "invalid_request");
    };
    dispatch_request(backend, &request)
}

/// 铸造 socket 路径：路径即凭据。win32 铸造随机命名管道（无口令连接），
/// posix 落临时目录 `.sock`；显式 `dir` 时两者都落该目录（TS `dir ?? tmpdir()`，
/// 分隔符随平台）。`random_hex` 对应 TS `randomBytes(8).toString("hex")`
/// （16 hex，不可枚举），由宿主注入随机源。
pub fn mint_broker_socket_path(
    random_hex: &str,
    dir: Option<&str>,
    tmp_dir: &str,
    is_windows: bool,
) -> String {
    if is_windows && dir.is_none() {
        return format!("\\\\.\\pipe\\mode-cua-{random_hex}");
    }
    let separator = if is_windows { '\\' } else { '/' };
    let base = dir.unwrap_or(tmp_dir);
    let base = base.trim_end_matches(['/', '\\']);
    format!("{base}{separator}mode-cua-{random_hex}.sock")
}

/// env 发现优先：进程树里已有注入的 Helper socket 时复用（services 的
/// `probeStableCuaHelperSocket` 与 macOS 懒启动注入都依赖这条语义）；
/// 没有注入才铸造新路径（路径即凭据）。注入值**原样返回不做 trim**
/// （TS 测试锁定「 X 」原样返回）；空白值不算注入。
pub fn resolve_broker_socket_path(
    env_value: Option<&str>,
    random_hex: &str,
    dir: Option<&str>,
    tmp_dir: &str,
    is_windows: bool,
) -> String {
    match env_value {
        Some(value) if !value.trim().is_empty() => value.to_string(),
        _ => mint_broker_socket_path(random_hex, dir, tmp_dir, is_windows),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn backend(method: &str, _params: &Value) -> BackendOutcome {
        match method {
            "health" => BackendOutcome::Ok(json!({ "pid": 7 })),
            _ => BackendOutcome::Err {
                message: format!("nope: {method}"),
                code: Some("not_settable".to_string()),
            },
        }
    }

    #[test]
    fn line_codec_roundtrip_and_rejects_garbage() {
        let request =
            parse_request_line(r#"{"id":"1","method":"health","params":{}}"#).expect("合法请求行");
        assert_eq!(request.method, "health");
        assert_eq!(request.id.as_deref(), Some("1"));
        assert!(parse_request_line("not json").is_none());
        assert!(parse_request_line(r#"{"method":1}"#).is_none());
        assert!(parse_request_line("[]").is_none());
        assert!(parse_request_line("null").is_none());
        // params 缺失 / null 都按 {}；id 非字符串按 null。
        let request = parse_request_line(r#"{"method":"observe"}"#).expect("无 params");
        assert_eq!(request.params, json!({}));
        assert_eq!(request.id, None);
        let request = parse_request_line(r#"{"id":5,"method":"observe","params":null}"#)
            .expect("null params");
        assert_eq!(request.params, json!({}));
        assert_eq!(request.id, None);
        // 序列化是紧凑 JSON，往返一致。
        let line = serialize_response(&ok_response(json!({ "pid": 7 })));
        assert_eq!(
            serde_json::from_str::<Value>(&line).expect("解析"),
            json!({ "ok": true, "result": { "pid": 7 } })
        );
    }

    #[test]
    fn method_whitelist_primitives_only_and_readonly_subset() {
        for method in BROKER_METHODS {
            assert!(is_broker_method(method), "{method}");
        }
        assert!(!is_broker_method("exec"));
        assert!(is_read_only_broker_method("observe"));
        assert!(!is_read_only_broker_method("perform"));
        // raw_input 是注入写路径：必须在方法表内、必须在只读表外。
        assert!(is_broker_method("raw_input") && !is_read_only_broker_method("raw_input"));
        // rd_media_* 同为写路径（起采集/换屏/收口管道）。
        assert!(
            is_broker_method("rd_media_start") && !is_read_only_broker_method("rd_media_start")
        );
        assert!(
            is_broker_method("rd_media_switch") && !is_read_only_broker_method("rd_media_switch")
        );
        assert!(is_broker_method("rd_media_stop") && !is_read_only_broker_method("rd_media_stop"));
    }

    #[test]
    fn dispatch_rejects_unknown_method_and_wraps_backend_failures() {
        let request = BrokerRequest {
            id: Some("1".to_string()),
            method: "exec".to_string(),
            params: json!({}),
        };
        assert_eq!(
            dispatch_request(&mut backend, &request),
            json!({ "ok": false, "error": { "code": "method_not_found", "message": "unknown method: exec" } })
        );
        let request = BrokerRequest {
            id: Some("2".to_string()),
            method: "health".to_string(),
            params: json!({}),
        };
        assert_eq!(
            dispatch_request(&mut backend, &request),
            json!({ "ok": true, "result": { "pid": 7 } })
        );
        let request = BrokerRequest {
            id: Some("3".to_string()),
            method: "perform".to_string(),
            params: json!({}),
        };
        assert_eq!(
            dispatch_request(&mut backend, &request),
            json!({ "ok": false, "error": { "code": "not_settable", "message": "nope: perform" } })
        );
    }

    #[test]
    fn backend_failure_without_code_falls_back_to_internal() {
        // 异常无 code / 空 code → internal（errorResponseFromException 口径）。
        let mut sink = |_: &str, _: &Value| BackendOutcome::Err {
            message: "boom".to_string(),
            code: None,
        };
        let request = BrokerRequest {
            id: None,
            method: "capture".to_string(),
            params: json!({}),
        };
        assert_eq!(
            dispatch_request(&mut sink, &request),
            json!({ "ok": false, "error": { "code": "internal", "message": "boom" } })
        );
    }

    #[test]
    fn handle_request_line_answers_invalid_request_for_malformed_input() {
        let mut sink = |_: &str, _: &Value| BackendOutcome::Ok(json!({}));
        assert_eq!(
            handle_request_line(&mut sink, "not json"),
            json!({ "ok": false, "error": { "code": "invalid_request", "message": "malformed request" } })
        );
        let ok = handle_request_line(&mut sink, r#"{"id":"1","method":"health"}"#);
        assert_eq!(ok, json!({ "ok": true, "result": {} }));
    }

    #[test]
    fn mint_uses_platform_naming_and_dir_override() {
        // win32 无 dir → 随机命名管道；有 dir → 落目录（与 posix 同路径形）。
        assert_eq!(
            mint_broker_socket_path("0123456789abcdef", None, "/tmp", true),
            "\\\\.\\pipe\\mode-cua-0123456789abcdef"
        );
        assert_eq!(
            mint_broker_socket_path("0123456789abcdef", None, "/tmp", false),
            "/tmp/mode-cua-0123456789abcdef.sock"
        );
        assert_eq!(
            mint_broker_socket_path("0123456789abcdef", Some("C:\\custom"), "/tmp", true),
            "C:\\custom\\mode-cua-0123456789abcdef.sock"
        );
        // 尾部分隔符不重复。
        assert_eq!(
            mint_broker_socket_path("0123456789abcdef", Some("/custom/"), "/tmp", false),
            "/custom/mode-cua-0123456789abcdef.sock"
        );
    }

    #[test]
    fn env_discovery_returns_untrimmed_and_falls_back_to_mint() {
        // 注入值原样返回，不做 trim（TS 测试锁定）。
        assert_eq!(
            resolve_broker_socket_path(Some(" X "), "0123456789abcdef", None, "/tmp", true),
            " X "
        );
        // 空白值不算注入 → 回落铸造。
        assert_eq!(
            resolve_broker_socket_path(Some("   "), "0123456789abcdef", None, "/tmp", true),
            "\\\\.\\pipe\\mode-cua-0123456789abcdef"
        );
        // 完全没有注入 → 回落铸造。
        assert_eq!(
            resolve_broker_socket_path(None, "0123456789abcdef", None, "/tmp", false),
            "/tmp/mode-cua-0123456789abcdef.sock"
        );
    }
}
