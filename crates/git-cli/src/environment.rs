//! Git 可执行文件探测与命令环境（`gitEnvironmentProvider.ts`）。

use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use crate::config::{git_binary_candidates, git_command_env, DEFAULT_GIT_DISCOVERY_TIMEOUT_MS};

pub struct GitEnvironmentProvider {
    candidates: Vec<String>,
    cached: Mutex<Option<Option<String>>>,
}

impl GitEnvironmentProvider {
    pub fn new() -> Self {
        Self::with_candidates(git_binary_candidates())
    }

    pub fn with_candidates(candidates: Vec<String>) -> Self {
        Self {
            candidates,
            cached: Mutex::new(None),
        }
    }

    /// 一次会话内只探测一次（`git --version` 的首次成功候选被缓存）。
    pub fn resolve_git_binary(&self) -> Option<String> {
        let mut cached = self.cached.lock().unwrap();
        if let Some(value) = cached.as_ref() {
            return value.clone();
        }
        let env = git_command_env();
        let resolved = self
            .candidates
            .iter()
            .find(|candidate| can_execute(candidate, &env))
            .cloned();
        *cached = Some(resolved.clone());
        resolved
    }

    /// 供 `Command` 使用的环境覆写（`None` 表示删除）。
    pub fn command_env(&self) -> Vec<(String, Option<String>)> {
        git_command_env()
    }
}

impl Default for GitEnvironmentProvider {
    fn default() -> Self {
        Self::new()
    }
}

fn can_execute(candidate: &str, env: &[(String, Option<String>)]) -> bool {
    let mut command = Command::new(candidate);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (name, value) in env {
        match value {
            Some(value) => {
                command.env(name, value);
            }
            None => {
                command.env_remove(name);
            }
        }
    }
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    let deadline = Instant::now() + Duration::from_millis(DEFAULT_GIT_DISCOVERY_TIMEOUT_MS);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {}
            Err(_) => return false,
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
}
