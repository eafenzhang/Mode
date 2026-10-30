//! Agent 运行时注册表：一个 workspace key 只允许一个 Agent 子进程。
//!
//! key 口径与仓库一致：`workspaceIdentity?.trim() || workspacePath`（远程身份优先）。
//! 这是所有权切换的硬前置——壳与 Host 不能同时为同一 workspace 拉起 Agent。
//!
//! 未做（下一片）：空闲看门狗自动回收、健康探测、provider/account 配置同步、进程换代
//! 事件。当前提供显式 `reap_idle` 与 `shutdown_all` 供宿主生命周期调用。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::Mutex as AsyncMutex;

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
    last_used: Instant,
}

pub struct AgentRuntimeRegistry {
    factory: SpecFactory,
    entries: Mutex<HashMap<String, Entry>>,
    /// 同一 key 的并发 `get_or_spawn` 只允许一个真正 spawn；不同 key 互不阻塞。
    spawn_locks: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
}

impl AgentRuntimeRegistry {
    pub fn new(factory: SpecFactory) -> Self {
        Self {
            factory,
            entries: Mutex::new(HashMap::new()),
            spawn_locks: Mutex::new(HashMap::new()),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.lock().expect("registry poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
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
            self.touch(&key);
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
            self.touch(&key);
            return Ok(process);
        }
        self.remove_stale(&key).await;

        let spec = (self.factory)(target)?;
        let spawned = spawn(spec).await?;
        let process = Arc::new(spawned.process);
        let mut entries = self.entries.lock().expect("registry poisoned");
        entries.insert(
            key,
            Entry {
                process: Arc::clone(&process),
                last_used: Instant::now(),
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
    pub async fn reap_idle(&self, older_than: Duration, grace: Duration) -> usize {
        let now = Instant::now();
        let stale_keys: Vec<String> = {
            let entries = self.entries.lock().expect("registry poisoned");
            entries
                .iter()
                .filter(|(_, entry)| now.duration_since(entry.last_used) >= older_than)
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

    fn touch(&self, key: &str) {
        if let Some(entry) = self
            .entries
            .lock()
            .expect("registry poisoned")
            .get_mut(key)
        {
            entry.last_used = Instant::now();
        }
    }

    async fn remove_stale(&self, key: &str) {
        let stale = self.entries.lock().expect("registry poisoned").remove(key);
        if let Some(entry) = stale {
            // 旧进程已经关闭时 shutdown 立即返回；仍存活则按零宽限强杀，避免双进程。
            entry.process.shutdown(Duration::ZERO).await;
        }
    }
}
