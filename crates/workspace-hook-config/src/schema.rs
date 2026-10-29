//! `hooks` 配置的严格校验（对齐 TS `workspaceHooksConfigSchema`）。
//!
//! zod 口径：根对象、`events` 对象、matcher 对象都是 `.strict()`（出现未知键即非法）；
//! 单条 hook 定义是 `.passthrough()`（未知键保留，供 digest 与运行时消费）。
//! 校验合法的输入原样保留，调用方据此落盘/计算摘要。

use serde_json::{Map, Value};

pub const WORKSPACE_HOOK_EVENT_NAMES: [&str; 7] = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "PostToolUseFailure",
    "Stop",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError(pub String);

impl std::fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl std::error::Error for ValidationError {}

fn fail(path: &str, message: &str) -> ValidationError {
    ValidationError(format!("{path}: {message}"))
}

fn is_finite_positive(value: &Value) -> bool {
    value
        .as_f64()
        .is_some_and(|number| number.is_finite() && number > 0.0)
}

fn non_empty_string(value: &Value) -> bool {
    value.as_str().is_some_and(|text| !text.is_empty())
}

fn check_optional_boolean(object: &Map<String, Value>, key: &str, path: &str) -> Result<(), ValidationError> {
    match object.get(key) {
        None => Ok(()),
        Some(Value::Bool(_)) => Ok(()),
        Some(_) => Err(fail(&format!("{path}.{key}"), "必须是布尔值")),
    }
}

fn check_optional_positive(object: &Map<String, Value>, key: &str, path: &str) -> Result<(), ValidationError> {
    match object.get(key) {
        None => Ok(()),
        Some(value) if is_finite_positive(value) => Ok(()),
        Some(_) => Err(fail(&format!("{path}.{key}"), "必须是有限正数")),
    }
}

fn check_optional_non_empty_string(
    object: &Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<(), ValidationError> {
    match object.get(key) {
        None => Ok(()),
        Some(value) if non_empty_string(value) => Ok(()),
        Some(_) => Err(fail(&format!("{path}.{key}"), "必须是非空字符串")),
    }
}

/// 校验一条 hook 定义（`type` 为 `process` 或 `command`）。
fn validate_hook_definition(value: &Value, path: &str) -> Result<(), ValidationError> {
    let Some(object) = value.as_object() else {
        return Err(fail(path, "必须是对象"));
    };
    let hook_type = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| fail(&format!("{path}.type"), "必须是 process 或 command"))?;
    match hook_type {
        "process" => {
            check_command(object, path)?;
            match object.get("args") {
                None => {}
                Some(Value::Array(items)) if items.iter().all(Value::is_string) => {}
                Some(_) => return Err(fail(&format!("{path}.args"), "必须是字符串数组")),
            }
            check_optional_positive(object, "timeoutMs", path)?;
            check_optional_non_empty_string(object, "statusMessage", path)?;
            Ok(())
        }
        "command" => {
            check_command(object, path)?;
            check_optional_boolean(object, "async", path)?;
            match object.get("shell") {
                None => {}
                Some(Value::Bool(true)) => {}
                Some(value) if non_empty_string(value) => {}
                Some(_) => {
                    return Err(fail(&format!("{path}.shell"), "必须是 true 或非空字符串"))
                }
            }
            check_optional_positive(object, "timeout", path)?;
            check_optional_positive(object, "timeoutMs", path)?;
            check_optional_non_empty_string(object, "statusMessage", path)?;
            Ok(())
        }
        other => Err(fail(
            &format!("{path}.type"),
            &format!("未知的 hook 类型 {other}"),
        )),
    }
}

fn check_command(object: &Map<String, Value>, path: &str) -> Result<(), ValidationError> {
    match object.get("command") {
        Some(value) if non_empty_string(value) => {}
        _ => return Err(fail(&format!("{path}.command"), "必须是非空字符串")),
    }
    check_optional_boolean(object, "enabled", path)
}

fn validate_matcher(value: &Value, path: &str) -> Result<(), ValidationError> {
    let Some(object) = value.as_object() else {
        return Err(fail(path, "必须是对象"));
    };
    // matcher 对象是 strict：只允许 matcher 与 hooks。
    for key in object.keys() {
        if key != "matcher" && key != "hooks" {
            return Err(fail(&format!("{path}.{key}"), "未知字段"));
        }
    }
    check_optional_non_empty_string(object, "matcher", path)?;
    let Some(hooks) = object.get("hooks") else {
        return Err(fail(&format!("{path}.hooks"), "缺失"));
    };
    let Some(hooks) = hooks.as_array() else {
        return Err(fail(&format!("{path}.hooks"), "必须是数组"));
    };
    if hooks.is_empty() {
        return Err(fail(&format!("{path}.hooks"), "至少需要一个 hook"));
    }
    for (index, hook) in hooks.iter().enumerate() {
        validate_hook_definition(hook, &format!("{path}.hooks[{index}]"))?;
    }
    Ok(())
}

/// 校验 `hooks` 配置根对象。合法时返回 `Ok(())`，输入原样可用。
pub fn validate_hooks_config(value: &Value) -> Result<(), ValidationError> {
    let Some(object) = value.as_object() else {
        return Err(fail("hooks", "必须是对象"));
    };
    for key in object.keys() {
        if !matches!(
            key.as_str(),
            "enabled" | "timeoutMs" | "maxOutputBytes" | "events"
        ) {
            return Err(fail(&format!("hooks.{key}"), "未知字段"));
        }
    }
    check_optional_boolean(object, "enabled", "hooks")?;
    check_optional_positive(object, "timeoutMs", "hooks")?;
    check_optional_positive(object, "maxOutputBytes", "hooks")?;
    if let Some(events) = object.get("events") {
        let Some(events) = events.as_object() else {
            return Err(fail("hooks.events", "必须是对象"));
        };
        for (name, matchers) in events {
            if !WORKSPACE_HOOK_EVENT_NAMES.contains(&name.as_str()) {
                return Err(fail(&format!("hooks.events.{name}"), "未知事件名"));
            }
            let Some(matchers) = matchers.as_array() else {
                return Err(fail(&format!("hooks.events.{name}"), "必须是数组"));
            };
            for (index, matcher) in matchers.iter().enumerate() {
                validate_matcher(matcher, &format!("hooks.events.{name}[{index}]"))?;
            }
        }
    }
    Ok(())
}
