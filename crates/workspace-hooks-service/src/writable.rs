//! UI `Hook` → 配置文件的写回投影（对齐 TS `getWritableHook` / `toModeHooksEvents` /
//! `resolveNextRootEnabled`）。

use serde_json::{Map, Value};

use crate::{custom_fields, hook_enabled, Hook};

fn truthy_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn truthy_number(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64).filter(|number| *number != 0.0)
}

/// 声明级开关：有 `configuredState` 时，只有「UI 开关与归并结果相同」才写回声明值，
/// 否则写回用户当前选择——保证「改根级开关不会顺手覆盖单条声明的显式选择」。
pub fn resolve_writable_declaration_enabled(hook: &Hook) -> bool {
    let enabled = hook_enabled(hook);
    let Some(state) = hook.get("configuredState").and_then(Value::as_object) else {
        return enabled;
    };
    let configured_enabled = state.get("configuredEnabled").and_then(Value::as_bool);
    if configured_enabled == Some(enabled) {
        state
            .get("declarationEnabled")
            .and_then(Value::as_bool)
            .unwrap_or(enabled)
    } else {
        enabled
    }
}

/// 单条 Hook 的落盘形态：`custom` 展开到顶层，`type`/`command`/`enabled` 覆盖同名自定义键。
pub fn get_writable_hook(hook: &Hook) -> Value {
    let mut out = custom_fields(hook);
    let hook_type = hook
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    out.insert("type".into(), Value::String(hook_type.clone()));
    out.insert(
        "command".into(),
        hook.get("command").cloned().unwrap_or(Value::String(String::new())),
    );
    out.insert(
        "enabled".into(),
        Value::Bool(resolve_writable_declaration_enabled(hook)),
    );
    if let Some(status_message) = truthy_string(hook.get("statusMessage")) {
        out.insert("statusMessage".into(), Value::String(status_message));
    }
    if hook_type == "command" {
        if hook.get("async").and_then(Value::as_bool) == Some(true) {
            out.insert("async".into(), Value::Bool(true));
        }
        if let Some(shell) = hook.get("shell").filter(|value| is_truthy_shell(value)) {
            out.insert("shell".into(), shell.clone());
        }
        if let Some(timeout) = truthy_number(hook.get("timeout")) {
            out.insert("timeout".into(), number_value(timeout));
        }
    } else {
        if let Some(args) = hook.get("args").and_then(Value::as_array) {
            if !args.is_empty() {
                out.insert("args".into(), Value::Array(args.clone()));
            }
        }
        if let Some(timeout) = truthy_number(hook.get("timeout")) {
            out.insert("timeoutMs".into(), number_value(timeout * 1000.0));
        }
    }
    Value::Object(out)
}

fn is_truthy_shell(value: &Value) -> bool {
    match value {
        Value::Bool(true) => true,
        Value::String(text) => !text.is_empty(),
        _ => false,
    }
}

fn number_value(value: f64) -> Value {
    if value.fract() == 0.0 && value.abs() < 9e15 {
        Value::from(value as i64)
    } else {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

/// 按事件分组、按 matcher 复用同一组；matcher 缺省时与同样缺省的组合并。
pub fn to_mode_hooks_events(hooks: &[Hook]) -> Value {
    let mut events = Map::new();
    for hook in hooks {
        let Some(event) = hook.get("event").and_then(Value::as_str) else {
            continue;
        };
        let matcher = hook.get("matcher").cloned().filter(|value| !value.is_null());
        let entry = events
            .entry(event.to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        let Some(matchers) = entry.as_array_mut() else {
            continue;
        };
        let index = matchers.iter().position(|item| {
            let existing = item.get("matcher").filter(|value| !value.is_null());
            existing == matcher.as_ref()
        });
        let target = match index {
            Some(index) => &mut matchers[index],
            None => {
                let mut object = Map::new();
                if let Some(matcher) = &matcher {
                    object.insert("matcher".into(), matcher.clone());
                }
                object.insert("hooks".into(), Value::Array(Vec::new()));
                matchers.push(Value::Object(object));
                matchers.last_mut().expect("刚 push 的 matcher")
            }
        };
        if let Some(hooks) = target.get_mut("hooks").and_then(Value::as_array_mut) {
            hooks.push(get_writable_hook(hook));
        }
    }
    Value::Object(events)
}

/// 根级 `enabled`：只要有一条「用户显式开过」的 Hook 就写成 true，否则沿用原值。
pub fn resolve_next_root_enabled(existing_enabled: Option<bool>, hooks: &[Hook]) -> Option<bool> {
    let has_explicit_enable = hooks.iter().any(|hook| {
        if !hook_enabled(hook) {
            return false;
        }
        let Some(state) = hook.get("configuredState").and_then(Value::as_object) else {
            return true;
        };
        state.get("configuredEnabled").and_then(Value::as_bool) != Some(true)
    });
    if has_explicit_enable {
        Some(true)
    } else {
        existing_enabled
    }
}
