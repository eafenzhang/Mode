//! Agent 运行时注册表：一个 workspace key 只允许一个 Agent 子进程。
//!
//! key 口径与仓库一致：`workspaceIdentity?.trim() || workspacePath`（远程身份优先）。
//! 这是所有权切换的硬前置——壳与 Host 不能同时为同一 workspace 拉起 Agent。
//!
//! 未做（下一片）：健康探测、provider/account 配置同步、进程换代事件。空闲回收由
//! [`AgentRuntimeRegistry::spawn_idle_reaper`] 提供周期任务，壳负责在换代/退出时 abort。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Mutex as AsyncMutex;

use crate::storage::{StorageStartupPhase, StorageStartupState};
use crate::{spawn, AgentProcess, AgentProcessError, AgentProcessSpec};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceTarget {
    pub workspace_path: String,
    pub workspace_identity: Option<String>,
}

impl WorkspaceTarget {
    pub fn new(workspace_path: impl Into<String>) -> Self {
        Self {
            workspace_path: workspace_path.into(),
            workspace_identity: None,
        }
    }

    pub fn with_identity(mut self, identity: impl Into<String>) -> Self {
        self.workspace_identity = Some(identity.into());
        self
    }

    /// `workspaceIdentity?.trim() || workspacePath`
    pub fn key(&self) -> String {
        match self.workspace_identity.as_deref().map(str::trim) {
            Some(identity) if !identity.is_empty() => identity.to_string(),
            _ => self.workspace_path.clone(),
        }
    }
}

pub type SpecFactory =
    Arc<dyn Fn(&WorkspaceTarget) -> Result<AgentProcessSpec, AgentProcessError> + Send + Sync>;

struct Entry {
    process: Arc<AgentProcess>,
    generation: u64,
    /// 拉起时的 workspace 身份；供 `syncAppRuntimePreferences` 等广播写入复用。
    target: WorkspaceTarget,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageStartupSnapshot {
    pub generation: u64,
    pub state: Option<StorageStartupState>,
}

/// 健康状态（W4 所有权切换 S0，只读）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeHealthState {
    Starting,
    Ready,
    /// 进程存活且 storage gate ready，但最近一次请求失败。
    /// 当前尚未接入请求失败计数，保留枚举值供后续填充。
    Degraded,
    Dead,
}

/// 单个 workspace 的健康快照；`None` 表示当前没有该 workspace 的运行时条目。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeHealth {
    pub state: RuntimeHealthState,
    pub workspace_key: String,
    /// `${workspaceKey}:${generation}:${pid}`，与 `get_runtime_identity` 同口径。
    pub runtime_identity: String,
    pub generation: u64,
    pub process_id: Option<u32>,
    pub storage_state: Option<StorageStartupState>,
    pub last_error: Option<String>,
}

/// 现有 runtime 的不透明身份；用于区分换代前后的订阅/命令。
///
/// `identity` 与 TS `ModeAgentRuntimeIdentity.identity` 同构：
/// `${workspaceKey}:${generation}:${pid}`（Rust 侧没有 lane 维度）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeIdentity {
    pub generation: u64,
    pub identity: String,
    pub process_id: Option<u32>,
    pub workspace_key: String,
}

/// `onAgentRuntimeRestarted` 事件：某 workspace 的新 runtime 已拉起（generation > 1）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeRestartedEvent {
    pub workspace_key: String,
    pub runtime_identity: RuntimeIdentity,
}

pub type RuntimeRestartListener = Arc<dyn Fn(&RuntimeRestartedEvent) + Send + Sync>;
pub type Unsubscribe = Box<dyn FnOnce() + Send + 'static>;

pub struct AgentRuntimeRegistry {
    factory: SpecFactory,
    next_generation: AtomicU64,
    entries: Mutex<HashMap<String, Entry>>,
    /// 同一 key 的并发 `get_or_spawn` 只允许一个真正 spawn；不同 key 互不阻塞。
    spawn_locks: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
    /// 入站请求处理器：对现有进程立即生效，并应用到之后拉起的进程。
    request_handler: Mutex<Option<mode_protocol_client::RequestHandler>>,
    /// 每个 workspace 的 spawn 次数（对应 TS `runtimeGenerationByWorkspaceKey`）。
    spawn_counts: Mutex<HashMap<String, u64>>,
    restart_listeners: Arc<Mutex<Vec<RuntimeRestartListener>>>,
}

impl AgentRuntimeRegistry {
    pub fn new(factory: SpecFactory) -> Self {
        Self {
            factory,
            next_generation: AtomicU64::new(0),
            entries: Mutex::new(HashMap::new()),
            spawn_locks: Mutex::new(HashMap::new()),
            request_handler: Mutex::new(None),
            spawn_counts: Mutex::new(HashMap::new()),
            restart_listeners: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 设置入站请求处理器：现有进程立即生效，之后拉起的进程自动挂上。
    /// 订阅 runtime 换代事件；返回退订句柄。
    pub fn on_runtime_restarted(&self, listener: RuntimeRestartListener) -> Unsubscribe {
        let listeners = Arc::clone(&self.restart_listeners);
        listeners
            .lock()
            .expect("restart listeners poisoned")
            .push(Arc::clone(&listener));
        Box::new(move || {
            let mut listeners = listeners.lock().expect("restart listeners poisoned");
            if let Some(index) = listeners
                .iter()
                .position(|candidate| Arc::ptr_eq(candidate, &listener))
            {
                listeners.remove(index);
            }
        })
    }

    pub fn set_request_handler(&self, handler: Option<mode_protocol_client::RequestHandler>) {
        *self
            .request_handler
            .lock()
            .expect("request handler poisoned") = handler.clone();
        let processes: Vec<Arc<AgentProcess>> = self
            .entries
            .lock()
            .expect("registry poisoned")
            .values()
            .map(|entry| Arc::clone(&entry.process))
            .collect();
        for process in processes {
            match &handler {
                Some(handler) => process.client().set_request_handler(Arc::clone(handler)),
                None => process.client().clear_request_handler(),
            }
        }
    }

    pub fn len(&self) -> usize {
        self.entries.lock().expect("registry poisoned").len()
    }

    /// 当前活进程对应的 workspace（去重靠 entries 的 key）。
    pub fn active_targets(&self) -> Vec<WorkspaceTarget> {
        self.entries
            .lock()
            .expect("registry poisoned")
            .values()
            .map(|entry| entry.target.clone())
            .collect()
    }

    /// 只读健康快照（S0）：不改动进程、不拉起。
    ///
    /// 判定：传输关闭或 storage gate 终态失败 → `dead`；storage gate 未 ready → `starting`；
    /// 其余 → `ready`。`degraded` 需要请求失败计数，当前不产生。
    pub fn health(&self, target: &WorkspaceTarget) -> Option<RuntimeHealth> {
        let key = target.key();
        let entries = self.entries.lock().expect("registry poisoned");
        let entry = entries.get(&key)?;
        let process_id = entry.process.pid();
        let gate = entry.process.storage_gate();
        let storage_state = gate.snapshot();
        let last_error = gate.terminal_error();
        let closed = entry.process.client().is_closed();
        let storage_ready = !gate.is_required()
            || storage_state
                .as_ref()
                .is_some_and(|snapshot| snapshot.phase == StorageStartupPhase::Ready);
        let state = if closed || last_error.is_some() {
            RuntimeHealthState::Dead
        } else if storage_ready {
            RuntimeHealthState::Ready
        } else {
            RuntimeHealthState::Starting
        };
        Some(RuntimeHealth {
            state,
            workspace_key: key.clone(),
            runtime_identity: format!("{key}:{}:{process_id}", entry.generation),
            generation: entry.generation,
            process_id: Some(process_id),
            storage_state,
            last_error,
        })
    }

    /// 当前没有任何活进程条目。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 现有活进程（existing-only，不 spawn）；没有时返回 `None`。
    pub fn existing_process(&self, target: &WorkspaceTarget) -> Option<Arc<AgentProcess>> {
        self.live_entry(&target.key())
    }

    /// 全部活 runtime 的 target/process 快照（宿主进程观测用）。
    pub fn live_processes(&self) -> Vec<(WorkspaceTarget, Arc<AgentProcess>)> {
        let entries = self.entries.lock().expect("registry poisoned");
        entries
            .values()
            .filter(|entry| !entry.process.client().is_closed())
            .map(|entry| (entry.target.clone(), Arc::clone(&entry.process)))
            .collect()
    }

    /// 任意一个活 runtime 的 workspace target（对应 TS usage 全局库的「任一已连接 client」）。
    pub fn first_live_target(&self) -> Option<WorkspaceTarget> {
        let entries = self.entries.lock().expect("registry poisoned");
        entries
            .values()
            .find(|entry| !entry.process.client().is_closed())
            .map(|entry| entry.target.clone())
    }

    /// 现有 runtime 的身份（existing-only，不 spawn）；没有活进程时返回 `None`。
    pub fn existing_identity(&self, target: &WorkspaceTarget) -> Option<RuntimeIdentity> {
        let key = target.key();
        let entries = self.entries.lock().expect("registry poisoned");
        let entry = entries
            .get(&key)
            .filter(|entry| !entry.process.client().is_closed())?;
        let process_id = entry.process.pid();
        Some(RuntimeIdentity {
            generation: entry.generation,
            identity: format!("{key}:{}:{process_id}", entry.generation),
            process_id: Some(process_id),
            workspace_key: key,
        })
    }

    /// 现有进程的存储启动快照（不 spawn）。没有活进程时返回 `None`。
    pub fn storage_startup_state(
        &self,
        target: &WorkspaceTarget,
    ) -> Option<StorageStartupSnapshot> {
        let key = target.key();
        let entries = self.entries.lock().expect("registry poisoned");
        let entry = entries
            .get(&key)
            .filter(|entry| !entry.process.client().is_closed())?;
        Some(StorageStartupSnapshot {
            generation: entry.generation,
            state: entry.process.storage_gate().snapshot(),
        })
    }

    /// 拉起（如需）并等待存储 ready；非 storage 入口立即返回。
    pub async fn prepare_storage(&self, target: &WorkspaceTarget) -> Result<(), String> {
        let process = self
            .get_or_spawn(target)
            .await
            .map_err(|error| error.to_string())?;
        process.storage_gate().wait_ready().await
    }

    /// 订阅现有进程的存储状态变化（不 spawn）。
    pub fn subscribe_storage_startup(
        &self,
        target: &WorkspaceTarget,
    ) -> Option<tokio::sync::watch::Receiver<Option<StorageStartupState>>> {
        let key = target.key();
        let entries = self.entries.lock().expect("registry poisoned");
        let entry = entries
            .get(&key)
            .filter(|entry| !entry.process.client().is_closed())?;
        Some(entry.process.storage_gate().subscribe())
    }

    /// 取（必要时拉起）该 workspace 的唯一 Agent。
    ///
    /// - 已存在且 transport 未关闭：复用；
    /// - 已存在但已关闭（Agent 退出/崩溃）：先回收再重建；
    /// - 同一 key 的并发调用只 spawn 一次。
    pub async fn get_or_spawn(
        &self,
        target: &WorkspaceTarget,
    ) -> Result<Arc<AgentProcess>, AgentProcessError> {
        let key = target.key();

        if let Some(process) = self.live_entry(&key) {
            return Ok(process);
        }

        let spawn_lock = {
            let mut locks = self.spawn_locks.lock().expect("spawn locks poisoned");
            Arc::clone(
                locks
                    .entry(key.clone())
                    .or_insert_with(|| Arc::new(AsyncMutex::new(()))),
            )
        };
        let _guard = spawn_lock.lock().await;

        // 等锁期间可能已有别的调用完成 spawn，或旧进程刚好关闭。
        if let Some(process) = self.live_entry(&key) {
            return Ok(process);
        }
        self.remove_stale(&key).await;

        let spec = (self.factory)(target)?;
        let spawned = spawn(spec).await?;
        let process = Arc::new(spawned.process);
        if let Some(handler) = self
            .request_handler
            .lock()
            .expect("request handler poisoned")
            .clone()
        {
            process.client().set_request_handler(handler);
        }
        let generation = self.next_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let pid = process.pid();
        {
            let mut entries = self.entries.lock().expect("registry poisoned");
            entries.insert(
                key.clone(),
                Entry {
                    process: Arc::clone(&process),
                    generation,
                    target: target.clone(),
                },
            );
        }
        let spawn_count = {
            let mut counts = self.spawn_counts.lock().expect("spawn counts poisoned");
            let count = counts.entry(key.clone()).or_insert(0);
            *count += 1;
            *count
        };
        if spawn_count > 1 {
            let event = RuntimeRestartedEvent {
                workspace_key: key.clone(),
                runtime_identity: RuntimeIdentity {
                    generation,
                    identity: format!("{key}:{generation}:{pid}"),
                    process_id: Some(pid),
                    workspace_key: key.clone(),
                },
            };
            let listeners: Vec<_> = self
                .restart_listeners
                .lock()
                .expect("restart listeners poisoned")
                .clone();
            for listener in listeners {
                listener(&event);
            }
        }
        Ok(process)
    }

    /// 关闭并移除指定 workspace 的 Agent（下次调用会重新拉起）。
    pub async fn invalidate(&self, workspace_key: &str, grace: Duration) {
        let entry = self
            .entries
            .lock()
            .expect("registry poisoned")
            .remove(workspace_key);
        if let Some(entry) = entry {
            entry.process.shutdown(grace).await;
        }
    }

    /// 回收空闲超过 `older_than` 的 Agent。返回回收数量。
    ///
    /// 空闲口径与 TS `scheduleIdleReclaim` 一致：**没有在飞请求**且距上次归零已超过阈值。
    /// 在飞请求未清空时不回收，等它结束、计时重新开始后再判断。
    pub async fn reap_idle(&self, older_than: Duration, grace: Duration) -> usize {
        let stale_keys: Vec<String> = {
            let entries = self.entries.lock().expect("registry poisoned");
            entries
                .iter()
                .filter(|(_, entry)| {
                    let client = entry.process.client();
                    client.pending_count() == 0 && client.idle_duration() >= older_than
                })
                .map(|(key, _)| key.clone())
                .collect()
        };
        let mut reaped = 0;
        for key in stale_keys {
            let entry = self.entries.lock().expect("registry poisoned").remove(&key);
            if let Some(entry) = entry {
                entry.process.shutdown(grace).await;
                reaped += 1;
            }
        }
        reaped
    }

    /// 启动周期性的空闲回收任务，返回任务句柄（调用方在壳换代/退出时 abort）。
    ///
    /// 首轮延后一个 `interval`，避免进程刚拉起就被判定空闲；`interval` 决定回收时间
    /// 粒度，因此应显著小于 `idle_timeout`。
    pub fn spawn_idle_reaper(
        self: &Arc<Self>,
        interval: Duration,
        idle_timeout: Duration,
        grace: Duration,
    ) -> tokio::task::JoinHandle<()> {
        let registry = Arc::clone(self);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // `interval` 的第一次 tick 立即返回；跳过它。
            ticker.tick().await;
            loop {
                ticker.tick().await;
                registry.reap_idle(idle_timeout, grace).await;
            }
        })
    }

    /// 壳退出/换代时关闭全部 Agent。
    pub async fn shutdown_all(&self, grace: Duration) {
        let entries: Vec<Entry> = {
            let mut guard = self.entries.lock().expect("registry poisoned");
            guard.drain().map(|(_, entry)| entry).collect()
        };
        for entry in entries {
            entry.process.shutdown(grace).await;
        }
    }

    fn live_entry(&self, key: &str) -> Option<Arc<AgentProcess>> {
        let entries = self.entries.lock().expect("registry poisoned");
        entries
            .get(key)
            .filter(|entry| !entry.process.client().is_closed())
            .map(|entry| Arc::clone(&entry.process))
    }

    async fn remove_stale(&self, key: &str) {
        let stale = self.entries.lock().expect("registry poisoned").remove(key);
        if let Some(entry) = stale {
            // 旧进程已经关闭时 shutdown 立即返回；仍存活则按零宽限强杀，避免双进程。
            entry.process.shutdown(Duration::ZERO).await;
        }
    }
}
