//! Windows 集成终端 shell 列表（`integratedTerminalShells.ts`）。
//!
//! 非 Windows 平台恒返回空数组。路径运算必须按 **win32 语义**做（即使进程跑在 Unix 上跑
//! 用例），因为 `PATH`/`PATHEXT`/`\` 分隔符都是目标平台的规则，不能用宿主的 `std::path`。

use std::collections::BTreeMap;

use crate::types::IntegratedTerminalShellOption;

const WINDOWS_GIT_BASH_PATHS: [&str; 2] = [
    "C:\\Program Files\\Git\\bin\\bash.exe",
    "C:\\Program Files (x86)\\Git\\bin\\bash.exe",
];

pub fn list_integrated_terminal_shells(
    env: &BTreeMap<String, String>,
    platform: &str,
    is_executable: &dyn Fn(&str) -> bool,
) -> Vec<IntegratedTerminalShellOption> {
    if platform != "win32" {
        return Vec::new();
    }

    let mut options = vec![command_prompt_option(env)];
    if let Some(git_bash) = resolve_windows_git_bash(env, is_executable) {
        options.push(IntegratedTerminalShellOption {
            dialect: "git-bash".into(),
            id: format!("git-bash:{}", git_bash.0),
            label: "Git Bash".into(),
            path: git_bash.0,
            source: git_bash.1.into(),
        });
    }
    options
}

fn command_prompt_option(env: &BTreeMap<String, String>) -> IntegratedTerminalShellOption {
    let path = env_get_ci(env, "ComSpec")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("cmd.exe")
        .to_string();
    IntegratedTerminalShellOption {
        dialect: "cmd".into(),
        id: format!("cmd:{path}"),
        label: "CMD".into(),
        path,
        source: "system".into(),
    }
}

/// `(path, source)`；source 是 `system`（固定路径）或 `path`（PATH 推断）。
fn resolve_windows_git_bash(
    env: &BTreeMap<String, String>,
    is_executable: &dyn Fn(&str) -> bool,
) -> Option<(String, &'static str)> {
    for candidate in WINDOWS_GIT_BASH_PATHS {
        if is_executable(candidate) {
            return Some((candidate.to_string(), "system"));
        }
    }

    let git_exe = windows_executable_candidates("git", env)
        .into_iter()
        .find(|candidate| is_executable(candidate))?;
    infer_windows_git_bash_paths_from_git_exe(&git_exe)
        .into_iter()
        .find(|candidate| is_executable(candidate))
        .map(|candidate| (candidate, "path"))
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

    let Some(path_value) = env_get_ci(env, "PATH").filter(|value| !value.is_empty()) else {
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
    let raw = env_get_ci(env, "PATHEXT").unwrap_or(".COM;.EXE;.BAT;.CMD");
    let mut extensions: Vec<String> = raw
        .split(';')
        .map(|extension| extension.trim().to_lowercase())
        .filter(|extension| !extension.is_empty())
        .collect();
    if !extensions.iter().any(|extension| extension == ".exe") {
        extensions.insert(0, ".exe".to_string());
    }
    extensions
}

/// Windows 环境变量名大小写不敏感（Node 侧同样按小写比较）。
pub(crate) fn env_get_ci<'a>(env: &'a BTreeMap<String, String>, key: &str) -> Option<&'a str> {
    let lower = key.to_lowercase();
    env.iter()
        .find(|(candidate, _)| candidate.to_lowercase() == lower)
        .map(|(_, value)| value.as_str())
}

/// `path.win32.dirname`：去掉末尾分隔符后取最后一个分隔符之前的部分。
fn win32_dirname(path: &str) -> String {
    let trimmed = path.trim_end_matches(['\\', '/']);
    match trimmed.rfind(['\\', '/']) {
        Some(index) if index > 0 => trimmed[..index].to_string(),
        Some(_) => trimmed[..1].to_string(),
        None => ".".to_string(),
    }
}

/// `path.win32.join` + `normalize` 的窄实现：拼 `\`，处理 `.`/`..`，保留盘符/UNC 前缀。
fn win32_join(parts: &[&str]) -> String {
    let joined = parts
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("\\");
    win32_normalize(&joined)
}

fn win32_normalize(path: &str) -> String {
    let unified = path.replace('/', "\\");
    let (prefix, rest) = split_windows_prefix(&unified);
    let mut stack: Vec<&str> = Vec::new();
    for segment in rest.split('\\') {
        match segment {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            other => stack.push(other),
        }
    }
    let joined = stack.join("\\");
    match prefix {
        Some(prefix) => {
            if joined.is_empty() {
                format!("{prefix}\\")
            } else {
                format!("{prefix}{joined}")
            }
        }
        None => joined,
    }
}

/// 返回 `(前缀, 剩余部分)`；前缀包含结尾分隔符（`C:\` / `\\\\server\\share\\`）。
fn split_windows_prefix(path: &str) -> (Option<String>, &str) {
    if let Some(rest) = path.strip_prefix("\\\\") {
        // UNC：前两段是 server 与 share。
        let mut index = 0;
        let mut seen = 0;
        for (position, ch) in rest.char_indices() {
            if ch == '\\' {
                seen += 1;
                if seen == 2 {
                    index = position + 1;
                    break;
                }
            }
        }
        if seen < 2 {
            return (Some(format!("\\\\{rest}")), "");
        }
        return (Some(format!("\\\\{}", &rest[..index])), &rest[index..]);
    }

    let bytes = path.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' {
        let drive = &path[..2];
        let rest = &path[2..];
        let rest = rest.strip_prefix('\\').unwrap_or(rest);
        return (Some(format!("{drive}\\")), rest);
    }

    (None, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    fn never(_: &str) -> bool {
        false
    }

    #[test]
    fn non_windows_platform_returns_empty() {
        assert!(list_integrated_terminal_shells(&env(&[]), "linux", &never).is_empty());
        assert!(list_integrated_terminal_shells(&env(&[]), "darwin", &never).is_empty());
    }

    #[test]
    fn cmd_uses_comspec_case_insensitively_and_trims() {
        let shells = list_integrated_terminal_shells(
            &env(&[("comspec", "  C:\\Windows\\system32\\cmd.exe  ")]),
            "win32",
            &never,
        );
        assert_eq!(shells.len(), 1);
        assert_eq!(shells[0].dialect, "cmd");
        assert_eq!(shells[0].path, "C:\\Windows\\system32\\cmd.exe");
        assert_eq!(shells[0].id, "cmd:C:\\Windows\\system32\\cmd.exe");
        assert_eq!(shells[0].source, "system");
    }

    #[test]
    fn cmd_falls_back_to_cmd_exe() {
        let shells = list_integrated_terminal_shells(&env(&[]), "win32", &never);
        assert_eq!(shells[0].path, "cmd.exe");
        assert_eq!(shells[0].label, "CMD");
    }

    #[test]
    fn fixed_git_bash_path_wins() {
        let fixed = WINDOWS_GIT_BASH_PATHS[0];
        let shells = list_integrated_terminal_shells(&env(&[]), "win32", &|path| path == fixed);
        assert_eq!(shells.len(), 2);
        assert_eq!(shells[1].dialect, "git-bash");
        assert_eq!(shells[1].path, fixed);
        assert_eq!(shells[1].source, "system");
        assert_eq!(shells[1].id, format!("git-bash:{fixed}"));
    }

    #[test]
    fn path_search_infers_bash_next_to_git() {
        // 用非固定安装目录，避免命中固定路径分支（固定路径优先是 TS 的既有顺序）。
        let env = env(&[
            ("PATH", "C:\\Windows\\system32;D:\\Tools\\Git\\cmd;"),
            ("PATHEXT", ".COM;.EXE"),
        ]);
        let git_exe = "D:\\Tools\\Git\\cmd\\git.exe";
        let bash = "D:\\Tools\\Git\\bin\\bash.exe";
        let shells = list_integrated_terminal_shells(&env, "win32", &|path| {
            path == git_exe || path == bash
        });
        assert_eq!(shells.len(), 2);
        assert_eq!(shells[1].path, bash);
        assert_eq!(shells[1].source, "path");
    }

    #[test]
    fn path_ext_defaults_and_always_has_exe() {
        let extensions = windows_executable_extensions(&env(&[("PATHEXT", ".COM;.CMD")]));
        assert_eq!(extensions, vec![".exe", ".com", ".cmd"]);
        let defaults = windows_executable_extensions(&env(&[]));
        assert_eq!(defaults, vec![".com", ".exe", ".bat", ".cmd"]);
    }

    #[test]
    fn win32_join_normalizes_dot_segments() {
        assert_eq!(
            win32_join(&["C:\\Program Files\\Git\\cmd", "..", "bin", "bash.exe"]),
            "C:\\Program Files\\Git\\bin\\bash.exe"
        );
        assert_eq!(
            win32_join(&["C:\\Program Files\\Git\\cmd", "..", "..", "bin", "bash.exe"]),
            // 两级 `..` 从 ...\Git\cmd 退到 ...\Program Files；这是 TS 第二个候选的原始语义
            // （它服务的是 git.exe 位于 <root>\bin 的布局），命中与否由可执行判定决定。
            "C:\\Program Files\\bin\\bash.exe"
        );
        assert_eq!(win32_join(&["C:", "..", "bin"]), "C:\\bin");
    }
}
