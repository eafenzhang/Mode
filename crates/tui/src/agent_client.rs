//! TUI 的 Agent stdio 客户端：spawn `agent-server app-server --stdio`，按行收发 JSON-RPC。
//!
//! 线程模型：主线程（crossterm 循环）只做 `request` / `try_recv`；reader 线程解析
//! stdout，并把 agent 的反向请求按最小口径应答，避免宿主等待超时。子进程退出时
//! Drop 负责 kill/wait。

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use serde_json::Value;

use crate::agent_wire::reverse_request_reply;

pub const AGENT_COMMAND_ENV: &str = "MODE_TUI_AGENT_COMMAND";
pub const AGENT_ARGS_ENV: &str = "MODE_TUI_AGENT_ARGS_JSON";
pub const DEFAULT_AGENT_COMMAND: &str = "agent-server";
pub const DEFAULT_AGENT_ARGS: [&str; 2] = ["app-server", "--stdio"];

pub struct AgentClient {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    receiver: mpsc::Receiver<Value>,
    next_id: AtomicU64,
}

impl AgentClient {
    /// 生产口径：`MODE_TUI_AGENT_COMMAND` + `MODE_TUI_AGENT_ARGS_JSON`（字符串数组）；
    /// 缺省 `agent-server app-server --stdio`。
    pub fn spawn_from_env() -> io::Result<Self> {
        let env: HashMap<String, String> = std::env::vars().collect();
        let (command, args) = command_from_env(&env)?;
        Self::spawn(&command, &args)
    }

    pub fn spawn(command: &str, args: &[String]) -> io::Result<Self> {
        Self::spawn_with_env(command, args, &[])
    }

    /// 显式 env 用于测试/嵌入（例如隔离 `MODE_DATA_BASE_DIR`）；生产走继承环境。
    pub fn spawn_with_env(
        command: &str,
        args: &[String],
        envs: &[(String, String)],
    ) -> io::Result<Self> {
        let mut process = Command::new(command);
        process
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // agent-server 的启动日志走 stderr；TUI 占着终端，不能让它污染画面。
            .stderr(Stdio::null());
        for (key, value) in envs {
            process.env(key, value);
        }
        let mut child = process.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("agent stdin unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("agent stdout unavailable"))?;
        let stdin = Arc::new(Mutex::new(stdin));
        let (sender, receiver) = mpsc::channel();
        let reply_stdin = Arc::clone(&stdin);
        thread::Builder::new()
            .name("mode-tui-agent-reader".to_string())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else {
                        break;
                    };
                    let Ok(value) = serde_json::from_str::<Value>(&line) else {
                        continue;
                    };
                    if let Some(reply) = reverse_request_reply(&value) {
                        let _ = write_json(&reply_stdin, &reply);
                        continue;
                    }
                    if sender.send(value).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            child,
            stdin,
            receiver,
            next_id: AtomicU64::new(1),
        })
    }

    pub fn request(&self, method: &str, params: Value) -> io::Result<u64> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        write_json(
            &self.stdin,
            &serde_json::json!({ "id": id, "method": method, "params": params }),
        )?;
        Ok(id)
    }

    pub fn try_recv(&self) -> Option<Value> {
        self.receiver.try_recv().ok()
    }
}

impl Drop for AgentClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn command_from_env(env: &HashMap<String, String>) -> io::Result<(String, Vec<String>)> {
    let command = env
        .get(AGENT_COMMAND_ENV)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_AGENT_COMMAND)
        .to_string();
    let args = match env
        .get(AGENT_ARGS_ENV)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        None => DEFAULT_AGENT_ARGS
            .iter()
            .map(|arg| arg.to_string())
            .collect(),
        Some(raw) => {
            let parsed: Vec<String> = serde_json::from_str(raw).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{AGENT_ARGS_ENV} must be a JSON string array: {error}"),
                )
            })?;
            if parsed.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{AGENT_ARGS_ENV} must not be empty"),
                ));
            }
            parsed
        }
    };
    Ok((command, args))
}

fn write_json(stdin: &Mutex<ChildStdin>, value: &Value) -> io::Result<()> {
    let mut guard = stdin
        .lock()
        .map_err(|_| io::Error::other("agent stdin poisoned"))?;
    serde_json::to_writer(&mut *guard, value).map_err(io::Error::other)?;
    guard.write_all(b"\n")?;
    guard.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn defaults_to_agent_server_app_server_stdio() {
        let (command, args) = command_from_env(&env(&[])).expect("defaults");
        assert_eq!(command, "agent-server");
        assert_eq!(args, vec!["app-server", "--stdio"]);
    }

    #[test]
    fn command_and_args_can_be_overridden() {
        let (command, args) = command_from_env(&env(&[
            (AGENT_COMMAND_ENV, " target/debug/agent-server.exe "),
            (AGENT_ARGS_ENV, r#"["app-server","--stdio","--flag"]"#),
        ]))
        .expect("override");
        assert_eq!(command, "target/debug/agent-server.exe");
        assert_eq!(args, vec!["app-server", "--stdio", "--flag"]);
    }

    #[test]
    fn invalid_args_fail_closed() {
        assert!(command_from_env(&env(&[(AGENT_ARGS_ENV, "not-json")])).is_err());
        assert!(command_from_env(&env(&[(AGENT_ARGS_ENV, "[]")])).is_err());
    }
}
