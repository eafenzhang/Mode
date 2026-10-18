//! sidecar 进程监督（M3 生态域 1 · Task 2 Step 3）。
//!
//! - spawn 子进程，stdout 逐行解码 NDJSON，按 `id` 把应答送回等待者；
//! - 请求带超时；超时只影响该请求，不杀进程；
//! - 子进程退出后，下一次请求按退避重启（初值/上限可配）；
//! - `Drop` 时 kill + wait，尽力不留孤儿。
//!
//! 协议本身在 [`crate::protocol`]；本模块只做进程与请求关联。

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::protocol::{decode_line, encode, Frame, Method, RpcError};

#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub request_timeout: Duration,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
}

impl SupervisorConfig {
    /// 开发形态：用当前 Node 跑 `entry`（测试里换成 `-e` 假脚本）。
    pub fn node(entry: impl Into<PathBuf>) -> Self {
        Self {
            program: PathBuf::from("node"),
            args: vec![entry.into().to_string_lossy().to_string()],
            request_timeout: Duration::from_secs(30),
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(2),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SupervisorError {
    Spawn(String),
    Io(String),
    Timeout,
    /// 子进程已退出且应答不会再来（包括崩溃）。
    Closed,
    Rpc(RpcError),
}

impl std::fmt::Display for SupervisorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(message) => write!(formatter, "启动插件宿主失败：{message}"),
            Self::Io(message) => write!(formatter, "插件宿主 IO 失败：{message}"),
            Self::Timeout => write!(formatter, "插件宿主请求超时"),
            Self::Closed => write!(formatter, "插件宿主已退出"),
            Self::Rpc(error) => write!(
                formatter,
                "插件宿主返回错误 {}：{}",
                error.code, error.message
            ),
        }
    }
}

impl std::error::Error for SupervisorError {}

struct State {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    next_backoff: Duration,
    last_spawn: Option<Instant>,
    /// 每次 spawn 递增；reader 只清理自己这一代的 pending，避免重启时误伤新请求。
    generation: u64,
}

type Pending = Arc<Mutex<HashMap<u64, (u64, Sender<Result<Value, RpcError>>)>>>;

pub struct SidecarSupervisor {
    config: SupervisorConfig,
    state: Mutex<State>,
    pending: Pending,
    /// reader 在 EOF 时记下自己那一代；`ensure_running` 用它识别「已经死了但还没被 reap」。
    reader_closed: Arc<Mutex<Option<u64>>>,
    next_id: AtomicU64,
    closed: AtomicBool,
}

impl SidecarSupervisor {
    pub fn start(config: SupervisorConfig) -> Result<Self, SupervisorError> {
        let supervisor = Self {
            config,
            state: Mutex::new(State {
                child: None,
                stdin: None,
                next_backoff: Duration::ZERO,
                last_spawn: None,
                generation: 0,
            }),
            pending: Arc::new(Mutex::new(HashMap::new())),
            reader_closed: Arc::new(Mutex::new(None)),
            next_id: AtomicU64::new(1),
            closed: AtomicBool::new(false),
        };
        supervisor.spawn()?;
        Ok(supervisor)
    }

    /// 发一条请求并等待应答。子进程已退出时按退避重启后再发。
    pub fn request(&self, method: Method, params: Value) -> Result<Value, SupervisorError> {
        self.request_with_timeout(method, params, self.config.request_timeout)
    }

    pub fn request_with_timeout(
        &self,
        method: Method,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, SupervisorError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(SupervisorError::Closed);
        }
        self.ensure_running()?;

        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (sender, receiver) = mpsc::channel();
        let frame = Frame::Request { id, method, params };

        let write_error = {
            let mut state = self.state.lock().expect("state poisoned");
            let generation = state.generation;
            self.pending
                .lock()
                .expect("pending poisoned")
                .insert(id, (generation, sender));
            match state.stdin.as_mut() {
                None => Some(SupervisorError::Closed),
                Some(stdin) => writeln!(stdin, "{}", encode(&frame))
                    .err()
                    .map(|error| SupervisorError::Io(error.to_string())),
            }
        };
        if let Some(error) = write_error {
            self.pending.lock().expect("pending poisoned").remove(&id);
            // 不自动重发：`plugin/callTool` 可能带副作用，重试是调用方的决定。
            self.invalidate_child();
            return Err(error);
        }

        match receiver.recv_timeout(timeout) {
            Ok(Ok(result)) => {
                self.state.lock().expect("state poisoned").next_backoff =
                    self.config.initial_backoff;
                Ok(result)
            }
            Ok(Err(error)) => Err(SupervisorError::Rpc(error)),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.pending.lock().expect("pending poisoned").remove(&id);
                Err(SupervisorError::Timeout)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.pending.lock().expect("pending poisoned").remove(&id);
                Err(SupervisorError::Closed)
            }
        }
    }

    /// 把当前子进程标记为失效（kill + wait），下一次请求会重新 spawn。
    fn invalidate_child(&self) {
        let mut state = self.state.lock().expect("state poisoned");
        state.stdin = None;
        if let Some(mut child) = state.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// 子进程已退出时按退避重启；活着就什么都不做。
    fn ensure_running(&self) -> Result<(), SupervisorError> {
        let mut state = self.state.lock().expect("state poisoned");
        let exited = match state.child.as_mut() {
            Some(child) => child
                .try_wait()
                .map_err(|error| SupervisorError::Io(error.to_string()))?
                .is_some(),
            None => true,
        };
        // EOF 可能早于 reap：reader 记下自己那一代，避免往一个已关闭的 stdin 写。
        let reader_dead =
            *self.reader_closed.lock().expect("reader flag poisoned") == Some(state.generation);
        if !exited && !reader_dead {
            return Ok(());
        }

        let backoff = state.next_backoff.max(self.config.initial_backoff);
        state.next_backoff = (backoff * 2).min(self.config.max_backoff);
        drop(state);
        if let Some(last_spawn) = self.state.lock().expect("state poisoned").last_spawn {
            let elapsed = last_spawn.elapsed();
            if elapsed < backoff {
                thread::sleep(backoff - elapsed);
            }
        }
        self.spawn()
    }

    fn spawn(&self) -> Result<(), SupervisorError> {
        let mut command = Command::new(&self.config.program);
        command
            .args(&self.config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| SupervisorError::Spawn(error.to_string()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| SupervisorError::Spawn("没有 stdin".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SupervisorError::Spawn("没有 stdout".to_string()))?;
        if let Some(stderr) = child.stderr.take() {
            thread::spawn(move || {
                let mut reader = BufReader::new(stderr);
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 {
                    line.clear();
                }
            });
        }

        let pending = Arc::clone(&self.pending);
        let reader_closed = Arc::clone(&self.reader_closed);
        let generation = {
            let mut state = self.state.lock().expect("state poisoned");
            state.generation += 1;
            state.generation
        };
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                match decode_line(&line) {
                    Ok(Frame::Response { id, result }) => {
                        if let Some((_, sender)) =
                            pending.lock().expect("pending poisoned").remove(&id)
                        {
                            let _ = sender.send(Ok(result));
                        }
                    }
                    Ok(Frame::Error { id, error }) => {
                        if let Some((_, sender)) =
                            pending.lock().expect("pending poisoned").remove(&id)
                        {
                            let _ = sender.send(Err(error));
                        }
                    }
                    Ok(Frame::Request { .. }) | Ok(Frame::Notification { .. }) => {
                        // sidecar 主动请求/通知不在本任务范围；忽略而不是拖死读取线程。
                    }
                    Err(_) => {
                        // 非法帧：同样忽略；宿主侧诊断由上层日志负责。
                    }
                }
            }
            // EOF：记下本代已关闭，并只让**本代**等待中的请求失败。
            *reader_closed.lock().expect("reader flag poisoned") = Some(generation);
            let mut keep = Vec::new();
            {
                let mut map = pending.lock().expect("pending poisoned");
                for (id, (entry_generation, sender)) in map.drain() {
                    if entry_generation == generation {
                        let _ = sender.send(Err(RpcError {
                            code: -32000,
                            message: "plugin host closed".to_string(),
                            data: None,
                        }));
                    } else {
                        keep.push((id, (entry_generation, sender)));
                    }
                }
                map.extend(keep);
            }
        });

        let mut state = self.state.lock().expect("state poisoned");
        if let Some(mut previous) = state.child.take() {
            let _ = previous.kill();
            let _ = previous.wait();
        }
        state.child = Some(child);
        state.stdin = Some(stdin);
        state.last_spawn = Some(Instant::now());
        Ok(())
    }
}

impl Drop for SidecarSupervisor {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
        if let Ok(mut state) = self.state.lock() {
            if let Some(mut child) = state.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}
