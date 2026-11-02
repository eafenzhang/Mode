//! 终端启动前的纯解析：shell、cwd、env patch、Windows PTY 信息与 ConPTY 回退判定。
//!
//! 真源：`packages/services/src/terminal/terminalService.ts` 的
//! `resolveTerminal*With` / `shouldFallbackFromConptyDllWith`。
//! 可执行/目录检查、平台、release、系统 home 与环境由调用方注入。

use indexmap::{IndexMap, IndexSet};
use regex::Regex;
use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalWindowsPtyInfo {
    pub build_number: Option<i64>,
}

fn is_missing_or_c_locale(value: Option<&str>) -> bool {
    let normalized = value.unwrap_or("").trim().to_uppercase();
    normalized.is_empty() || normalized == "C" || normalized == "POSIX"
}

fn is_utf8_locale(value: Option<&str>) -> bool {
    let pattern = Regex::new(r"(?i)utf-?8").expect("UTF-8 locale 正则可编译");
    value.is_some_and(|value| pattern.is_match(value))
}

pub fn parse_windows_build_number(release_text: &str) -> Option<i64> {
    let build_text = release_text.split('.').nth(2)?;
    let pattern = Regex::new(r"^[+-]?\d+").expect("build number 正则可编译");
    let captures = pattern.captures(build_text)?;
    captures[0].parse::<i64>().ok()
}

pub fn resolve_terminal_windows_pty_info_with(
    platform: &str,
    release_text: &str,
) -> Option<TerminalWindowsPtyInfo> {
    if platform != "win32" {
        return None;
    }
    Some(TerminalWindowsPtyInfo {
        build_number: parse_windows_build_number(release_text),
    })
}

pub fn should_fallback_from_conpty_dll_with(error: &str) -> bool {
    let pattern = Regex::new(
        r"(?i)conpty\.node module handle|conpty\.node module file name|cannot find conpty\.dll|error code:\s*126",
    )
    .expect("ConPTY 错误正则可编译");
    pattern.is_match(error)
}

const DARWIN_GUI_FALLBACK_PATHS: [&str; 8] = [
    "/opt/homebrew/bin",
    "/opt/homebrew/sbin",
    "/usr/local/bin",
    "/usr/local/sbin",
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
];

fn merge_path_entries(entries: &[Option<&str>], delimiter: char) -> String {
    let mut seen = IndexSet::new();
    for entry in entries.iter().flatten() {
        for item in entry.split(delimiter) {
            let trimmed = item.trim();
            if trimmed.is_empty() || !seen.insert(trimmed.to_string()) {
                continue;
            }
        }
    }
    seen.into_iter()
        .collect::<Vec<_>>()
        .join(&delimiter.to_string())
}

fn resolve_darwin_terminal_path(env: &IndexMap<String, String>, path_delimiter: char) -> String {
    let mut entries: Vec<Option<&str>> = vec![env.get("PATH").map(String::as_str)];
    for path in DARWIN_GUI_FALLBACK_PATHS {
        entries.push(Some(path));
    }
    merge_path_entries(&entries, path_delimiter)
}

fn resolve_fallback_utf8_locale(platform: &str, env: &IndexMap<String, String>) -> String {
    for value in [env.get("LC_ALL"), env.get("LC_CTYPE"), env.get("LANG")] {
        let text = value.map(String::as_str);
        if is_utf8_locale(text) {
            return text.expect("已检查 UTF-8").to_string();
        }
    }
    if platform == "darwin" {
        "en_US.UTF-8".to_string()
    } else {
        "C.UTF-8".to_string()
    }
}

pub fn resolve_terminal_env_with(
    platform: &str,
    env: &IndexMap<String, String>,
    path_delimiter: char,
) -> IndexMap<String, String> {
    let mut next_env = env.clone();
    let fallback_locale = resolve_fallback_utf8_locale(platform, env);
    if platform == "darwin" {
        next_env.insert(
            "PATH".to_string(),
            resolve_darwin_terminal_path(env, path_delimiter),
        );
    }
    next_env.insert("TERM".to_string(), "xterm-256color".to_string());
    let colorterm = env
        .get("COLORTERM")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "truecolor".to_string());
    next_env.insert("COLORTERM".to_string(), colorterm);
    if next_env.get("CI").map(String::as_str) == Some("1")
        && env.get("TERM").map(String::as_str) == Some("dumb")
    {
        next_env.shift_remove("CI");
    }
    if is_missing_or_c_locale(next_env.get("LANG").map(String::as_str)) {
        next_env.insert("LANG".to_string(), fallback_locale.clone());
    }
    if is_missing_or_c_locale(next_env.get("LC_CTYPE").map(String::as_str)) {
        next_env.insert("LC_CTYPE".to_string(), fallback_locale.clone());
    }
    if let Some(lc_all) = next_env.get("LC_ALL").map(String::as_str) {
        if is_missing_or_c_locale(Some(lc_all)) {
            next_env.insert("LC_ALL".to_string(), fallback_locale);
        }
    }
    next_env
}

fn windows_shell_candidates(env: &IndexMap<String, String>) -> Vec<String> {
    vec![
        "pwsh.exe".to_string(),
        "powershell.exe".to_string(),
        env.get("ComSpec").cloned().unwrap_or_default(),
        "cmd.exe".to_string(),
    ]
}

fn unix_shell_candidates(env: &IndexMap<String, String>) -> Vec<String> {
    vec![
        env.get("SHELL").cloned().unwrap_or_default(),
        "/bin/zsh".to_string(),
        "/bin/bash".to_string(),
        "/bin/sh".to_string(),
    ]
}

pub fn resolve_terminal_shell_with(
    platform: &str,
    env: &IndexMap<String, String>,
    is_executable: impl Fn(&str) -> bool,
) -> Result<String, String> {
    let candidates = if platform == "win32" {
        windows_shell_candidates(env)
    } else {
        unix_shell_candidates(env)
    };
    candidates
        .iter()
        .find(|candidate| !candidate.is_empty() && is_executable(candidate))
        .cloned()
        .ok_or_else(|| {
            if platform == "win32" {
                "No usable Windows shell found for terminal startup".to_string()
            } else {
                "No usable shell found for terminal startup".to_string()
            }
        })
}

pub fn resolve_terminal_cwd_with(
    cwd: Option<&str>,
    env: &IndexMap<String, String>,
    os_home: &str,
    is_usable_directory: impl Fn(&str) -> bool,
) -> Result<String, String> {
    let candidates = [
        cwd.filter(|value| !value.is_empty()).map(str::to_string),
        env.get("HOME").cloned(),
        (!os_home.is_empty()).then(|| os_home.to_string()),
        Some("/".to_string()),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|candidate| !candidate.is_empty() && is_usable_directory(candidate))
        .ok_or_else(|| "No usable working directory found for terminal startup".to_string())
}

pub fn windows_pty_info_to_json(info: &TerminalWindowsPtyInfo) -> Value {
    let mut entries = vec![("backend".to_string(), json!("conpty"))];
    if let Some(build_number) = info.build_number {
        entries.push(("buildNumber".to_string(), json!(build_number)));
    }
    Value::Object(entries.into_iter().collect())
}

pub fn terminal_env_to_json(env: &IndexMap<String, String>) -> Value {
    Value::Object(
        env.iter()
            .map(|(key, value)| (key.clone(), json!(value)))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_pty_info_parses_build_number() {
        let info = resolve_terminal_windows_pty_info_with("win32", "10.0.19045").expect("win32");
        assert_eq!(info.build_number, Some(19045));
        assert!(resolve_terminal_windows_pty_info_with("linux", "1.2.3").is_none());
    }

    #[test]
    fn conpty_errors_are_selective() {
        assert!(should_fallback_from_conpty_dll_with(
            "Cannot find conpty.dll"
        ));
        assert!(!should_fallback_from_conpty_dll_with("shell not found"));
    }

    #[test]
    fn darwin_env_adds_path_and_locale() {
        let mut env = IndexMap::new();
        env.insert("COLORTERM".to_string(), "  ".to_string());
        env.insert("CI".to_string(), "1".to_string());
        env.insert("TERM".to_string(), "dumb".to_string());
        let next = resolve_terminal_env_with("darwin", &env, ':');
        assert!(next["PATH"].starts_with("/opt/homebrew/bin:"));
        assert_eq!(next["TERM"], "xterm-256color");
        assert_eq!(next["COLORTERM"], "truecolor");
        assert!(!next.contains_key("CI"));
        assert_eq!(next["LANG"], "en_US.UTF-8");
    }
}
