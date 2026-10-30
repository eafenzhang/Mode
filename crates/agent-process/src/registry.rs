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

use crate::storage::StorageStartupState;
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

pub type SpecFactory = Arc<
    dyn Fn(&WorkspaceTarget) -> Result<AgentProcessSpec, AgentProcessError> + Send + Sync,
>;

struct Entry {
    process: Arc<AgentProcess>,
    generation: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StorageStartupSnapshot {
    pub generation: u64,
    pub state: Option<StorageStartupState>,
}

/// 现有 runtime 的不透明身份；用于区分换代前后的订阅/命令。
///
/// `identity` 与 TS `ModeAgentRuntimeIdentity.identity` 同构：
/// `${workspaceKey}:${generation}:${pid}`（Rust 侧没有 lane 维度）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeIdentity {
    pub generation: u64,
    pub identity: String,
    pub process_id: Option<u32>,
    pub workspace_key: String,
}

pub struct AgentRuntimeRegistry {
    factory: SpecFactory,
    next_generation: AtomicU64,
    entries: Mutex<HashMap<String, Entry>>,
    /// 同一 key 的并发 `get_or_spawn` 只允许一个真正 spawn；不同 key 互不阻塞。
    spawn_locks: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
}

impl AgentRuntimeRegistry {
    pub fn new(factory: SpecFactory) -> Self {
        Self {
            factory,
            next_generation: AtomicU64::new(0),
            entries: Mutex::new(HashMap::new()),
            spawn_locks: Mutex::new(HashMap::new()),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.lock().expect("registry poisoned").len()
    }

    /// 当前没有任何活进程条目。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
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
        let generation = self.next_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let mut entries = self.entries.lock().expect("registry poisoned");
        entries.insert(
            key,
            Entry {
                process: Arc::clone(&process),
                generation,
            },
        );
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
            let entry = self
                .entries
                .lock()
                .expect("registry poisoned")
                .remove(&key);
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
