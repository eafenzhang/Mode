//! 终端系统 profile 检测的纯解析面。
//!
//! 真源：`packages/services/src/terminal/terminalProfile.ts` 的
//! `parseJsonc` 与 `detect*FontFamilyFromRaw`。文件读取由调用方注入，
//! 环境变量不再读进程。

use std::collections::BTreeMap;

use regex::Regex;
use serde_json::Value;

use crate::TerminalDetectedProfile;

type RawFileReader<'a> = dyn Fn(&str) -> Option<String> + 'a;

fn env_value(env: &BTreeMap<String, String>, key: &str) -> Option<String> {
    let lower_key = key.to_lowercase();
    env.iter()
        .find(|(env_key, _)| env_key.to_lowercase() == lower_key)
        .and_then(|(_, value)| {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        })
}

fn home_dir(env: &BTreeMap<String, String>) -> String {
    env_value(env, "HOME")
        .or_else(|| env_value(env, "USERPROFILE"))
        .unwrap_or_default()
}

fn object(value: &Value) -> Option<&serde_json::Map<String, Value>> {
    value.as_object()
}

fn remove_json_trailing_commas(raw: &str) -> String {
    let mut result = String::with_capacity(raw.len());
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in raw.char_indices() {
        if in_string {
            result.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
            result.push(ch);
            continue;
        }
        if ch == ',' {
            let rest = &raw[index + ch.len_utf8()..];
            let next = rest.trim_start().chars().next();
            if matches!(next, Some('}') | Some(']')) {
                continue;
            }
        }
        result.push(ch);
    }
    result
}

fn strip_json_comments(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut result = String::with_capacity(raw.len());
    let mut index = 0;
    let mut in_string = false;
    let mut escaped = false;
    while index < chars.len() {
        let ch = chars[index];
        let next = chars.get(index + 1).copied();
        if in_string {
            result.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        if ch == '"' {
            in_string = true;
            result.push(ch);
            index += 1;
        } else if ch == '/' && next == Some('/') {
            while index < chars.len() && chars[index] != '\n' {
                index += 1;
            }
            result.push('\n');
        } else if ch == '/' && next == Some('*') {
            index += 2;
            while index + 1 < chars.len() && !(chars[index] == '*' && chars[index + 1] == '/') {
                index += 1;
            }
            index += 1;
        } else {
            result.push(ch);
            index += 1;
        }
    }
    result
}

/// 与 TS `parseJsonc` 一致：先尝试原始 JSON，失败后剥注释/尾逗号再试；只接受对象。
pub fn parse_jsonc(raw: &str) -> Option<Value> {
    let candidates = [
        raw.to_string(),
        remove_json_trailing_commas(&strip_json_comments(raw)),
    ];
    for candidate in candidates {
        if let Ok(value) = serde_json::from_str::<Value>(&candidate) {
            if value.is_object() {
                return Some(value);
            }
        }
    }
    None
}

pub fn read_terminal_config_path(value: &Value, path_segments: &[&str]) -> Option<String> {
    let mut current = value;
    for segment in path_segments {
        current = object(current)?.get(*segment)?;
    }
    crate::normalize_font_family(current.as_str())
}

/// Node `path.win32.join` 的窄口径：统一 `\`、折叠重复分隔符、保留驱动器/前导 `\`。
fn path_string(parts: &[&str]) -> String {
    let raw = parts
        .iter()
        .map(|part| part.replace('/', "\\"))
        .collect::<Vec<_>>()
        .join("\\");
    let mut result = String::with_capacity(raw.len());
    let mut previous_separator = false;
    for ch in raw.chars() {
        if ch == '\\' {
            if !previous_separator {
                result.push('\\');
                previous_separator = true;
            }
        } else {
            result.push(ch);
            previous_separator = false;
        }
    }
    result
}

pub fn detect_windows_terminal_font_family_from_raw(
    platform: &str,
    env: &BTreeMap<String, String>,
    raw_reader: &RawFileReader<'_>,
) -> Option<TerminalDetectedProfile> {
    if platform != "win32" {
        return None;
    }
    let local_app_data = env_value(env, "LOCALAPPDATA").or_else(|| env_value(env, "APPDATA"))?;
    let candidates = [
        path_string(&[
            &local_app_data,
            "Packages",
            "Microsoft.WindowsTerminal_8wekyb3d8bbwe",
            "LocalState",
            "settings.json",
        ]),
        path_string(&[
            &local_app_data,
            "Packages",
            "Microsoft.WindowsTerminalPreview_8wekyb3d8bbwe",
            "LocalState",
            "settings.json",
        ]),
        path_string(&[
            &local_app_data,
            "Microsoft",
            "Windows Terminal",
            "settings.json",
        ]),
    ];
    for file_path in candidates {
        let Some(raw) = raw_reader(&file_path) else {
            continue;
        };
        let Some(parsed) = parse_jsonc(&raw) else {
            continue;
        };
        let default_profile_id = read_terminal_config_path(&parsed, &["defaultProfile"]);
        let profiles = parsed.get("profiles");
        let list = profiles
            .and_then(|profiles| profiles.get("list"))
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        if let Some(default_profile_id) = default_profile_id {
            for item in list {
                let Some(profile) = item.as_object() else {
                    continue;
                };
                if profile.get("guid").and_then(Value::as_str) != Some(default_profile_id.as_str())
                {
                    continue;
                }
                if let Some(font_family) = item
                    .get("font")
                    .and_then(|font| font.get("face"))
                    .and_then(Value::as_str)
                    .and_then(|value| crate::normalize_font_family(Some(value)))
                {
                    return Some(TerminalDetectedProfile {
                        font_family: Some(font_family),
                        font_size: None,
                        theme: None,
                    });
                }
            }
        }
        let defaults = profiles.and_then(|profiles| profiles.get("defaults"));
        if let Some(font_family) = defaults
            .and_then(|defaults| defaults.get("font"))
            .and_then(|font| font.get("face"))
            .and_then(Value::as_str)
            .and_then(|value| crate::normalize_font_family(Some(value)))
        {
            return Some(TerminalDetectedProfile {
                font_family: Some(font_family),
                font_size: None,
                theme: None,
            });
        }
        for item in list {
            if let Some(font_family) = item
                .get("font")
                .and_then(|font| font.get("face"))
                .and_then(Value::as_str)
                .and_then(|value| crate::normalize_font_family(Some(value)))
            {
                return Some(TerminalDetectedProfile {
                    font_family: Some(font_family),
                    font_size: None,
                    theme: None,
                });
            }
        }
    }
    None
}

pub fn detect_vs_code_terminal_font_family_from_raw(
    env: &BTreeMap<String, String>,
    raw_reader: &RawFileReader<'_>,
) -> Option<TerminalDetectedProfile> {
    let home = home_dir(env);
    let app_data = env_value(env, "APPDATA");
    let xdg_config_home = env_value(env, "XDG_CONFIG_HOME");
    let mut candidates: Vec<String> = Vec::new();
    if let Some(app_data) = &app_data {
        candidates.push(path_string(&[app_data, "Code", "User", "settings.json"]));
        candidates.push(path_string(&[
            app_data,
            "Code - Insiders",
            "User",
            "settings.json",
        ]));
    }
    if let Some(xdg_config_home) = &xdg_config_home {
        candidates.push(path_string(&[
            xdg_config_home,
            "Code",
            "User",
            "settings.json",
        ]));
        candidates.push(path_string(&[
            xdg_config_home,
            "Code - Insiders",
            "User",
            "settings.json",
        ]));
    }
    candidates.push(path_string(&[
        &home,
        ".config",
        "Code",
        "User",
        "settings.json",
    ]));
    candidates.push(path_string(&[
        &home,
        ".config",
        "Code - Insiders",
        "User",
        "settings.json",
    ]));
    candidates.push(path_string(&[
        &home,
        "Library",
        "Application Support",
        "Code",
        "User",
        "settings.json",
    ]));
    candidates.push(path_string(&[
        &home,
        "Library",
        "Application Support",
        "Code - Insiders",
        "User",
        "settings.json",
    ]));
    for file_path in candidates {
        let Some(raw) = raw_reader(&file_path) else {
            continue;
        };
        let Some(parsed) = parse_jsonc(&raw) else {
            continue;
        };
        if let Some(font_family) =
            read_terminal_config_path(&parsed, &["terminal.integrated.fontFamily"])
        {
            return Some(TerminalDetectedProfile {
                font_family: Some(font_family),
                font_size: None,
                theme: None,
            });
        }
    }
    None
}

pub fn detect_kitty_font_family_from_raw(
    env: &BTreeMap<String, String>,
    raw_reader: &RawFileReader<'_>,
) -> Option<TerminalDetectedProfile> {
    let home = home_dir(env);
    let xdg_config_home =
        env_value(env, "XDG_CONFIG_HOME").unwrap_or_else(|| path_string(&[&home, ".config"]));
    let candidates = [
        path_string(&[&xdg_config_home, "kitty", "kitty.conf"]),
        path_string(&[
            &home,
            "Library",
            "Application Support",
            "kitty",
            "kitty.conf",
        ]),
    ];
    let pattern = Regex::new(r"(?m)^\s*font_family\s+(.+)$").expect("kitty 正则可编译");
    for file_path in candidates {
        let Some(raw) = raw_reader(&file_path) else {
            continue;
        };
        let Some(captures) = pattern.captures(&raw) else {
            continue;
        };
        let Some(font_family) = crate::normalize_font_family(Some(&captures[1])) else {
            continue;
        };
        let mut stripped = font_family.as_str();
        if stripped.starts_with('"') {
            stripped = &stripped[1..];
        }
        if stripped.ends_with('"') {
            stripped = &stripped[..stripped.len() - 1];
        }
        return Some(TerminalDetectedProfile {
            font_family: Some(stripped.to_string()),
            font_size: None,
            theme: None,
        });
    }
    None
}

fn parse_toml_object(raw: &str) -> Option<Value> {
    let value = toml::from_str::<toml::Value>(raw).ok()?;
    serde_json::to_value(value)
        .ok()
        .filter(|value| value.is_object())
}

fn parse_yaml_object(raw: &str) -> Option<Value> {
    let value = serde_yaml::from_str::<serde_yaml::Value>(raw).ok()?;
    serde_json::to_value(value)
        .ok()
        .filter(|value| value.is_object())
}

pub fn detect_alacritty_font_family_from_raw(
    env: &BTreeMap<String, String>,
    raw_reader: &RawFileReader<'_>,
) -> Option<TerminalDetectedProfile> {
    let home = home_dir(env);
    let xdg_config_home =
        env_value(env, "XDG_CONFIG_HOME").unwrap_or_else(|| path_string(&[&home, ".config"]));
    let toml_candidates = [
        path_string(&[&xdg_config_home, "alacritty", "alacritty.toml"]),
        path_string(&[&home, ".alacritty.toml"]),
    ];
    let yaml_candidates = [
        path_string(&[&xdg_config_home, "alacritty", "alacritty.yml"]),
        path_string(&[&xdg_config_home, "alacritty", "alacritty.yaml"]),
    ];
    for file_path in toml_candidates {
        let Some(raw) = raw_reader(&file_path) else {
            continue;
        };
        let Some(parsed) = parse_toml_object(&raw) else {
            continue;
        };
        if let Some(font_family) = read_terminal_config_path(&parsed, &["font", "normal", "family"])
        {
            return Some(TerminalDetectedProfile {
                font_family: Some(font_family),
                font_size: None,
                theme: None,
            });
        }
    }
    for file_path in yaml_candidates {
        let Some(raw) = raw_reader(&file_path) else {
            continue;
        };
        let Some(parsed) = parse_yaml_object(&raw) else {
            continue;
        };
        if let Some(font_family) = read_terminal_config_path(&parsed, &["font", "normal", "family"])
        {
            return Some(TerminalDetectedProfile {
                font_family: Some(font_family),
                font_size: None,
                theme: None,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_jsonc_comments_and_trailing_commas() {
        let raw = r#"{
  // comment
  "a": 1,
  "b": { "c": ["x", "y",], },
}"#;
        eprintln!("stripped={:?}", strip_json_comments(raw));
        eprintln!(
            "commas={:?}",
            remove_json_trailing_commas(&strip_json_comments(raw))
        );
        let parsed = parse_jsonc(raw).expect("JSONC 对象");
        assert_eq!(parsed["a"], 1);
        assert_eq!(parsed["b"]["c"][1], "y");
    }
}
