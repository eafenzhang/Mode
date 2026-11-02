//! Windows 集成终端 shell 选项探测。
//!
//! 真源：`packages/services/src/system/integratedTerminalShells.ts`。
//! 可执行检查由调用方注入；本 crate 不访问文件系统。

use std::collections::BTreeMap;

use serde_json::{json, Value};

const WINDOWS_GIT_BASH_PATHS: [&str; 2] = [
    r"C:\Program Files\Git\bin\bash.exe",
    r"C:\Program Files (x86)\Git\bin\bash.exe",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalShellDialect {
    Cmd,
    GitBash,
}

impl TerminalShellDialect {
    fn as_str(self) -> &'static str {
        match self {
            Self::Cmd => "cmd",
            Self::GitBash => "git-bash",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalShellSource {
    System,
    Path,
}

impl TerminalShellSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Path => "path",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntegratedTerminalShellOption {
    pub dialect: TerminalShellDialect,
    pub id: String,
    pub label: String,
    pub path: String,
    pub source: TerminalShellSource,
}

pub fn list_integrated_terminal_shell_options(
    env: &BTreeMap<String, String>,
    platform: &str,
    is_executable: impl Fn(&str) -> bool,
) -> Vec<IntegratedTerminalShellOption> {
    if platform != "win32" {
        return Vec::new();
    }
    let mut options = vec![command_prompt_option(env)];
    if let Some(git_bash) = resolve_windows_git_bash(env, &is_executable) {
        options.push(IntegratedTerminalShellOption {
            dialect: TerminalShellDialect::GitBash,
            id: format!("git-bash:{}", git_bash.0),
            label: "Git Bash".to_string(),
            path: git_bash.0,
            source: git_bash.1,
        });
    }
    options
}

fn command_prompt_option(env: &BTreeMap<String, String>) -> IntegratedTerminalShellOption {
    let path = windows_env_value(env, "ComSpec")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("cmd.exe")
        .to_string();
    IntegratedTerminalShellOption {
        dialect: TerminalShellDialect::Cmd,
        id: format!("cmd:{path}"),
        label: "CMD".to_string(),
        path,
        source: TerminalShellSource::System,
    }
}

fn resolve_windows_git_bash(
    env: &BTreeMap<String, String>,
    is_executable: &impl Fn(&str) -> bool,
) -> Option<(String, TerminalShellSource)> {
    for candidate in WINDOWS_GIT_BASH_PATHS {
        if is_executable(candidate) {
            return Some((candidate.to_string(), TerminalShellSource::System));
        }
    }
    let git_exe = windows_executable_candidates("git", env)
        .into_iter()
        .find(|candidate| is_executable(candidate))?;
    let inferred = infer_windows_git_bash_paths_from_git_exe(&git_exe)
        .into_iter()
        .find(|candidate| is_executable(candidate))?;
    Some((inferred, TerminalShellSource::Path))
}

fn infer_windows_git_bash_paths_from_git_exe(git_exe: &str) -> Vec<String> {
    let git_dir = win32_dirname(git_exe);
    vec![
        win32_join(&[&git_dir, "..", "bin", "bash.exe"]),
        win32_join(&[&git_dir, "..", "..", "bin", "bash.exe"]),
    ]
}

fn windows_executable_candidates(command: &str, env: &BTreeMap<String, String>) -> Vec<String> {
    if command.contains('\\') || command.contains('/') {
        return vec![command.to_string()];
    }
    let Some(path_value) = windows_env_value(env, "PATH") else {
        return vec![command.to_string()];
    };
    let extensions = windows_executable_extensions(env);
    path_value
        .split(';')
        .filter(|entry| !entry.trim().is_empty())
        .flat_map(|entry| {
            extensions
                .iter()
                .map(move |extension| win32_join(&[entry, &format!("{command}{extension}")]))
        })
        .collect()
}

fn windows_executable_extensions(env: &BTreeMap<String, String>) -> Vec<String> {
    let raw_extensions = windows_env_value(env, "PATHEXT")
        .map(|value| {
            value
                .split(';')
                .map(|extension| extension.trim().to_lowercase())
                .filter(|extension| !extension.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| {
            vec![
                ".com".to_string(),
                ".exe".to_string(),
                ".bat".to_string(),
                ".cmd".to_string(),
            ]
        });
    if raw_extensions.iter().any(|extension| extension == ".exe") {
        raw_extensions
    } else {
        let mut result = vec![".exe".to_string()];
        result.extend(raw_extensions);
        result
    }
}

fn windows_env_value<'a>(env: &'a BTreeMap<String, String>, key: &str) -> Option<&'a str> {
    let lower_key = key.to_lowercase();
    env.iter()
        .find(|(env_key, _)| env_key.to_lowercase() == lower_key)
        .map(|(_, value)| value.as_str())
}

fn win32_dirname(path: &str) -> String {
    let normalized = path.replace('/', "\\");
    let trimmed = normalized.trim_end_matches('\\');
    let trimmed = if trimmed.is_empty() && normalized.starts_with('\\') {
        "\\"
    } else {
        trimmed
    };
    match trimmed.rfind('\\') {
        None => ".".to_string(),
        Some(0) => "\\".to_string(),
        Some(index) => {
            let prefix = &trimmed[..index];
            if prefix.len() == 2 && prefix.ends_with(':') {
                format!("{prefix}\\")
            } else {
                prefix.to_string()
            }
        }
    }
}

fn win32_join(parts: &[&str]) -> String {
    let raw = parts
        .iter()
        .map(|part| part.replace('/', "\\"))
        .collect::<Vec<_>>()
        .join("\\");
    let (drive, rest, absolute) =
        if raw.len() >= 2 && raw.as_bytes()[0].is_ascii_alphabetic() && raw.as_bytes()[1] == b':' {
            (
                raw[..2].to_string(),
                raw[2..].to_string(),
                raw[2..].starts_with('\\'),
            )
        } else {
            (String::new(), raw.clone(), raw.starts_with('\\'))
        };
    let mut components: Vec<&str> = Vec::new();
    for component in rest.split('\\') {
        match component {
            "" | "." => {}
            ".." => {
                if components.last().is_some_and(|last| *last != "..") {
                    components.pop();
                } else if !absolute {
                    components.push("..");
                }
            }
            value => components.push(value),
        }
    }
    let joined = components.join("\\");
    let prefix = if absolute { "\\" } else { "" };
    format!("{drive}{prefix}{joined}")
}

impl IntegratedTerminalShellOption {
    pub fn to_json(&self) -> Value {
        json!({
            "dialect": self.dialect.as_str(),
            "id": self.id,
            "label": self.label,
            "path": self.path,
            "source": self.source.as_str(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infers_git_bash_from_path_git() {
        let mut env = BTreeMap::new();
        env.insert("ComSpec".to_string(), "C:\\Windows\\cmd.exe".to_string());
        env.insert(
            "PATH".to_string(),
            "C:\\Tools\\Git\\cmd;C:\\Other".to_string(),
        );
        env.insert("PATHEXT".to_string(), ".EXE;.CMD".to_string());
        let existing = [
            "C:\\Tools\\Git\\cmd\\git.exe",
            "C:\\Tools\\Git\\bin\\bash.exe",
        ];
        let options =
            list_integrated_terminal_shell_options(&env, "win32", |path| existing.contains(&path));
        assert_eq!(options.len(), 2);
        assert_eq!(options[1].path, "C:\\Tools\\Git\\bin\\bash.exe");
        assert_eq!(options[1].source, TerminalShellSource::Path);
    }

    #[test]
    fn non_windows_is_empty() {
        assert!(
            list_integrated_terminal_shell_options(&BTreeMap::new(), "linux", |_| true).is_empty()
        );
    }
}
