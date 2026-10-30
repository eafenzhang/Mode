//! Agent 子进程生命周期（W4 最终态第一层）。
//!
//! 负责：拉起 `app-server --stdio` 形态的子进程、把 stdio 接到
//! [`mode_protocol_client::ProtocolClient`]、广播 stderr、优雅关闭（stdin EOF → 等待）
//! 与整树强杀。
//!
//! **不负责**：workspace → 进程注册表、空闲回收、provider 配置同步、`mode-agent`
//! 频道；这些在后续切片里做。当前也不实现 TS 的 stderr drain 窗口与进程树快照清理，
//! 先以「taskkill /T」或 Unix 进程组信号覆盖孤儿风险。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mode_protocol_client::{ClientOptions, ProtocolClient};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, Mutex};
use tokio::task::JoinHandle;

mod registry;
mod control;
mod storage;
mod provider_sync;
mod resolve;

pub use control::{
    build_grant_params, build_workspace_ref, AgentControlClient, AgentWorkspaceTarget,
    ControlError, GrantTrustReasonCode, GrantTrustResult,
    WORKSPACE_HOOK_TRUST_GRANT_METHOD,
};
pub use provider_sync::{
    AccountProviderConfigSnapshot, ProviderConfigSync, ProviderReadFuture, ProviderSource,
    ProviderSyncOutcome, ProviderSyncStatus, PROVIDER_UPDATE_ACCOUNT_CONFIG_METHOD,
};
pub use registry::{
    AgentRuntimeRegistry, RuntimeIdentity, SpecFactory, StorageStartupSnapshot, WorkspaceTarget,
};
pub use storage::{
    StorageDatabaseKind, StorageStartupGate, StorageStartupPhase, StorageStartupState,
    DEFAULT_FIRST_STATUS_TIMEOUT, STORAGE_STATE_NOTIFICATION,
};
pub use resolve::{
    resolve_agent_command, AgentCommandSource, AgentResolveContext, AgentResolveError,
    ResolvedAgentCommand,
};

const STDERR_CHANNEL_CAPACITY: usize = 256;
const KILL_REAP_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
pub struct AgentProcessSpec {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub request_timeout: Duration,
    /// 该入口是否上报 `startup/storageState`；为 true 时 `prepare_storage` 会等待 ready。
    pub supports_storage_startup: bool,
}

impl AgentProcessSpec {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            request_timeout: mode_protocol_client::DEFAULT_REQUEST_TIMEOUT,
            supports_storage_startup: false,
        }
    }
}

#[derive(Debug)]
pub enum AgentProcessError {
    Spawn { command: String, message: String },
}

impl std::fmt::Display for AgentProcessError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn { command, message } => {
                write!(formatter, "failed to spawn agent process `{command}`: {message}")
            }
        }
    }
}

impl std::error::Error for AgentProcessError {}

pub struct AgentProcess {
    client: ProtocolClient,
    child: Mutex<Option<Child>>,
    pid: u32,
    stderr_task: JoinHandle<()>,
    storage_task: JoinHandle<()>,
    storage_gate: Arc<StorageStartupGate>,
}

impl AgentProcess {
    pub fn client(&self) -> &ProtocolClient {
        &self.client
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn storage_gate(&self) -> &Arc<StorageStartupGate> {
        &self.storage_gate
    }

    /// 优雅关闭：关闭写端（对端收到 stdin EOF），最多等 `grace`；超时则整树强杀。
    pub async fn shutdown(&self, grace: Duration) {
        self.client.close_write();
        let exited = {
            let mut guard = self.child.lock().await;
            match guard.as_mut() {
                Some(child) => tokio::time::timeout(grace, child.wait()).await.is_ok(),
                None => true,
            }
        };
        if !exited {
            self.kill_tree().await;
        }
        self.stderr_task.abort();
        self.storage_task.abort();
    }

    /// 立即整树强杀并回收根进程。
    pub async fn kill(&self) {
        self.kill_tree().await;
        self.stderr_task.abort();
        self.storage_task.abort();
    }

    async fn kill_tree(&self) {
        let mut guard = self.child.lock().await;
        let Some(child) = guard.as_mut() else {
            return;
        };
        terminate_tree(child, self.pid).await;
        let _ = tokio::time::timeout(KILL_REAP_TIMEOUT, child.wait()).await;
        *guard = None;
    }
}

pub struct SpawnedAgent {
    pub process: AgentProcess,
    pub stderr: broadcast::Receiver<String>,
}

pub async fn spawn(spec: AgentProcessSpec) -> Result<SpawnedAgent, AgentProcessError> {
    let mut command = Command::new(&spec.command);
    command
        .args(&spec.args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // 子进程意外被丢弃时至少回收根进程；整树清理由 shutdown/kill 负责。
        .kill_on_drop(true);
    if let Some(cwd) = &spec.cwd {
        command.current_dir(cwd);
    }
    for (key, value) in &spec.env {
        command.env(key, value);
    }
    configure_platform(&mut command);

    let mut child = command.spawn().map_err(|error| AgentProcessError::Spawn {
        command: spec.command.clone(),
        message: error.to_string(),
    })?;
    let pid = child.id().unwrap_or_default();
    let stdin = child.stdin.take().expect("stdin piped");
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let (client, mut notifications) = ProtocolClient::connect_with_receiver(
        stdout,
        stdin,
        ClientOptions {
            request_timeout: spec.request_timeout,
        },
    );
    let storage_gate = Arc::new(StorageStartupGate::new(
        spec.supports_storage_startup,
        DEFAULT_FIRST_STATUS_TIMEOUT,
    ));
    let storage_task = {
        let gate = Arc::clone(&storage_gate);
        tokio::spawn(async move {
            loop {
                match notifications.recv().await {
                    Ok(message) => {
                        if message.get("method").and_then(|value| value.as_str())
                            == Some(STORAGE_STATE_NOTIFICATION)
                        {
                            if let Some(params) = message.get("params") {
                                gate.accept(params);
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        })
    };

    let (stderr_tx, stderr_rx) = broadcast::channel(STDERR_CHANNEL_CAPACITY);
    let stderr_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = stderr_tx.send(line);
        }
    });

    Ok(SpawnedAgent {
        process: AgentProcess {
            client,
            child: Mutex::new(Some(child)),
            pid,
            stderr_task,
            storage_task,
            storage_gate,
        },
        stderr: stderr_rx,
    })
}

/// GUI 壳拉起控制台子程序时不能弹窗；Unix 上让子进程成为进程组组长，便于整组终止。
fn configure_platform(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.as_std_mut().creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = command;
    }
}

async fn terminate_tree(child: &mut Child, pid: u32) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut command = Command::new("taskkill");
        command
            .args([
                "/PID".to_string(),
                pid.to_string(),
                "/T".to_string(),
                "/F".to_string(),
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .as_std_mut()
            .creation_flags(CREATE_NO_WINDOW);
        let _ = command.status().await;
    }
    #[cfg(unix)]
    {
        // 子进程在 spawn 时是进程组组长（pgid == pid），负 pid 终止整组。
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let _ = child.start_kill();
}

/// 测试/诊断用：进程组或任务是否仍在（不暴露给业务）。
#[doc(hidden)]
pub fn pid_is_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut command = std::process::Command::new("tasklist");
        command
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .creation_flags(CREATE_NO_WINDOW);
        match command.output() {
            Ok(output) => String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\"")),
            Err(_) => true,
        }
    }
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(any(windows, unix)))]
    {
        false
    }
}

