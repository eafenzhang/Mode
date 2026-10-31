//! Git 命令执行（`gitCommandProvider.ts`）：超时、强制结束、输出上限与截断。

use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::config::{git_command_env, DEFAULT_GIT_COMMAND_TIMEOUT_MS, DEFAULT_GIT_OUTPUT_BYTES};
use crate::environment::GitEnvironmentProvider;

const TIMEOUT_KILL_GRACE_MS: u64 = 2_000;
const TIMEOUT_FORCE_KILL_GRACE_MS: u64 = 2_000;
const WINDOWS_TASKKILL_TIMEOUT_MS: u64 = 2_000;
const READ_CHUNK_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct GitCommandExecutionOptions {
    pub cwd: PathBuf,
    pub args: Vec<String>,
    pub timeout_ms: Option<u64>,
    pub max_output_bytes: Option<usize>,
    /// 额外环境变量（`None` = 删除该变量）。
    pub env: Vec<(String, Option<String>)>,
}

impl GitCommandExecutionOptions {
    pub fn new(cwd: PathBuf, args: Vec<String>) -> Self {
        Self {
            cwd,
            args,
            timeout_ms: None,
            max_output_bytes: None,
            env: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct GitCommandExecutionResult {
    pub binary_path: String,
    pub cwd: String,
    pub args: Vec<String>,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub signal: Option<String>,
    pub duration_ms: u64,
    pub timed_out: bool,
    pub timeout_ms: Option<u64>,
    pub timeout_elapsed_ms: Option<u64>,
    pub timeout_close_delay_ms: Option<u64>,
    pub force_kill_attempted: bool,
    pub orphaned: bool,
    pub output_truncated: bool,
}

pub struct GitCommandProvider {
    environment: Arc<GitEnvironmentProvider>,
    /// 每次都叠加的环境（测试用于固定作者/时间与隔离全局配置）。
    extra_env: Vec<(String, Option<String>)>,
}

impl GitCommandProvider {
    pub fn new(environment: Arc<GitEnvironmentProvider>) -> Self {
        Self {
            environment,
            extra_env: Vec::new(),
        }
    }

    pub fn with_extra_env(mut self, extra_env: Vec<(String, Option<String>)>) -> Self {
        self.extra_env = extra_env;
        self
    }

    pub fn resolve_git_binary(&self) -> Option<String> {
        self.environment.resolve_git_binary()
    }

    pub fn run(
        &self,
        options: &GitCommandExecutionOptions,
    ) -> Result<GitCommandExecutionResult, String> {
        let Some(binary_path) = self.environment.resolve_git_binary() else {
            return Err("Git binary is not available".to_string());
        };
        let timeout_ms = options.timeout_ms.unwrap_or(DEFAULT_GIT_COMMAND_TIMEOUT_MS);
        let max_output_bytes = options.max_output_bytes.unwrap_or(DEFAULT_GIT_OUTPUT_BYTES);
        let started_at = Instant::now();

        let mut command = Command::new(&binary_path);
        command
            .args(&options.args)
            .current_dir(&options.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (name, value) in git_command_env() {
            match value {
                Some(value) => {
                    command.env(name, value);
                }
                None => {
                    command.env_remove(name);
                }
            }
        }
        for (name, value) in self.extra_env.iter().chain(options.env.iter()) {
            match value {
                Some(value) => {
                    command.env(name, value);
                }
                None => {
                    command.env_remove(name);
                }
            }
        }

        let mut child = match command.spawn() {
            Ok(child) => child,
            // Node 的 `error` 事件分支：降级成失败结果，不向上抛异常。
            Err(error) => {
                return Ok(GitCommandExecutionResult {
                    binary_path,
                    cwd: options.cwd.to_string_lossy().into_owned(),
                    args: options.args.clone(),
                    stdout: String::new(),
                    stderr: error.to_string(),
                    exit_code: Some(-2),
                    signal: None,
                    duration_ms: elapsed_ms(started_at),
                    timed_out: false,
                    timeout_ms: Some(timeout_ms),
                    timeout_elapsed_ms: None,
                    timeout_close_delay_ms: None,
                    force_kill_attempted: false,
                    orphaned: false,
                    output_truncated: false,
                });
            }
        };

        let stdout_buffer: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let stderr_buffer: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let stdout_bytes = Arc::new(Mutex::new(0usize));
        let stderr_bytes = Arc::new(Mutex::new(0usize));
        let truncated = Arc::new(AtomicBool::new(false));

        let stdout_reader = spawn_reader(
            child.stdout.take(),
            Arc::clone(&stdout_buffer),
            Arc::clone(&stdout_bytes),
            Arc::clone(&truncated),
            max_output_bytes,
        );
        let stderr_reader = spawn_reader(
            child.stderr.take(),
            Arc::clone(&stderr_buffer),
            Arc::clone(&stderr_bytes),
            Arc::clone(&truncated),
            max_output_bytes,
        );

        let mut timed_out = false;
        let mut timeout_elapsed_ms = None;
        let mut force_kill_attempted = false;
        let mut orphaned = false;
        let deadline = Duration::from_millis(timeout_ms);
        let exit_status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {}
                Err(_) => {}
            }
            if truncated.load(Ordering::Relaxed) {
                let _ = child.kill();
            }
            if started_at.elapsed() >= deadline {
                timed_out = true;
                timeout_elapsed_ms = Some(elapsed_ms(started_at));
                kill_child(&mut child);
                if wait_for_exit(&mut child, TIMEOUT_KILL_GRACE_MS) {
                    break child.try_wait().ok().flatten();
                }
                force_kill_attempted = true;
                force_kill_child(&child);
                if wait_for_exit(&mut child, TIMEOUT_FORCE_KILL_GRACE_MS) {
                    break child.try_wait().ok().flatten();
                }
                orphaned = true;
                break None;
            }
            thread::sleep(Duration::from_millis(10));
        };

        if orphaned {
            // 子进程仍在运行：跳过 join，避免管道读取线程阻塞主链路（与 TS 的 orphaned 语义一致）。
            drop(stdout_reader);
            drop(stderr_reader);
        } else {
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
        }

        let duration_ms = elapsed_ms(started_at);
        let stdout = String::from_utf8_lossy(&stdout_buffer.lock().unwrap()).into_owned();
        let stderr = String::from_utf8_lossy(&stderr_buffer.lock().unwrap()).into_owned();
        Ok(GitCommandExecutionResult {
            binary_path,
            cwd: options.cwd.to_string_lossy().into_owned(),
            args: options.args.clone(),
            stdout,
            stderr,
            exit_code: exit_status.and_then(|status| status.code()),
            signal: None,
            duration_ms,
            timed_out,
            timeout_ms: Some(timeout_ms),
            timeout_elapsed_ms,
            timeout_close_delay_ms: if timed_out {
                timeout_elapsed_ms.map(|elapsed| duration_ms.saturating_sub(elapsed))
            } else {
                None
            },
            force_kill_attempted,
            orphaned,
            output_truncated: truncated.load(Ordering::Relaxed),
        })
    }
}

fn spawn_reader<R: Read + Send + 'static>(
    stream: Option<R>,
    buffer: Arc<Mutex<Vec<u8>>>,
    counter: Arc<Mutex<usize>>,
    truncated: Arc<AtomicBool>,
    max_output_bytes: usize,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let Some(mut stream) = stream else {
            return;
        };
        let mut chunk = vec![0u8; READ_CHUNK_BYTES];
        loop {
            let read = match stream.read(&mut chunk) {
                Ok(0) => return,
                Ok(read) => read,
                Err(_) => return,
            };
            if truncated.load(Ordering::Relaxed) {
                return;
            }
            let mut counter = counter.lock().unwrap();
            if *counter + read > max_output_bytes {
                truncated.store(true, Ordering::Relaxed);
                return;
            }
            *counter += read;
            buffer.lock().unwrap().extend_from_slice(&chunk[..read]);
        }
    })
}

fn wait_for_exit(child: &mut Child, timeout_ms: u64) -> bool {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) => {}
            Err(_) => return true,
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn kill_child(child: &mut Child) {
    let _ = child.kill();
}

/// Windows 上 Git helper/filter 可能拖住句柄：用 `taskkill /T /F` 结束整棵树。
fn force_kill_child(child: &Child) {
    #[cfg(windows)]
    {
        let mut command = Command::new("taskkill");
        command
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Ok(mut killer) = command.spawn() {
            let deadline = Instant::now() + Duration::from_millis(WINDOWS_TASKKILL_TIMEOUT_MS);
            loop {
                match killer.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => {}
                    Err(_) => break,
                }
                if Instant::now() >= deadline {
                    let _ = killer.kill();
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = child;
    }
}

fn elapsed_ms(started_at: Instant) -> u64 {
    started_at.elapsed().as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_binary_is_an_error_not_a_result() {
        let provider =
            GitCommandProvider::new(Arc::new(GitEnvironmentProvider::with_candidates(vec![
                "definitely-not-git-binary".to_string(),
            ])));
        let error = provider
            .run(&GitCommandExecutionOptions::new(
                std::env::temp_dir(),
                vec!["--version".to_string()],
            ))
            .unwrap_err();
        assert_eq!(error, "Git binary is not available");
    }
}
