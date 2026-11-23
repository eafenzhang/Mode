//! transport 分派器：把 `RemoteTransportPort` 的调用按 **target kind** 路由到具体端口。
//!
//! 规范见 `docs/specs/remote-workspaces-channel.md` §6。
//!
//! 为什么需要它：`RemoteChannel` 只认一个 `RemoteTransportPort`，而目标有五种 kind，
//! 阻塞点各不相同（LAN 已完成；SSH/WSL/Docker 需 SSH 客户端；v3 需 Noise 实现）。
//! 没有分派器时只能二选一——要么被迫给所有 kind 共用同一个端口，要么翻面后所有非
//! LAN 目标都落到同一条含糊错误上。
//!
//! 三条记账要求：
//!
//! 1. **`entry_id` → kind 必须记下来**。`abort`/`dispose`/`release_workspace` 只拿到
//!    entry id；没有 kind 就不知道该问哪个端口。不能靠 target 反推——session 生命
//!    周期里 target 未必在手边。
//! 2. **未注册 kind 的错误要准确**，说清「这种传输还没落地」而不是「远程服务未就绪」
//!    ——后者会让用户以为是配对或网络问题。
//! 3. **路由记录在 abort/dispose 后必须清掉**，否则 entry id 会无限累积。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use remote_session_registry::{ConnectPlan, RemoteTargetSpec, RemoteWorkspaceContext};
use rpc_host::BoxFuture;

use crate::remote::RemoteTransportPort;

type PortFactory = Arc<dyn Fn() -> Arc<dyn RemoteTransportPort> + Send + Sync>;
type EntryKinds = Arc<Mutex<HashMap<u64, &'static str>>>;

/// 按 kind 分派的 transport 端口组合。
#[derive(Clone)]
pub struct RemoteTransportRouter {
    ports: HashMap<&'static str, PortFactory>,
    entries: EntryKinds,
}

impl Default for RemoteTransportRouter {
    fn default() -> Self {
        Self {
            ports: HashMap::new(),
            entries: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl std::fmt::Debug for RemoteTransportRouter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut kinds: Vec<&'static str> = self.ports.keys().copied().collect();
        kinds.sort_unstable();
        formatter
            .debug_struct("RemoteTransportRouter")
            .field("kinds", &kinds)
            .field(
                "entries",
                &self.entries.lock().map(|map| map.len()).unwrap_or_default(),
            )
            .finish()
    }
}

impl RemoteTransportRouter {
    pub fn new() -> Self {
        Self::default()
    }

    /// 按 kind 注册端口（kind 由装配方显式给出，避免端口与装配对「支持哪种」分歧）。
    pub fn with_port(mut self, kind: &'static str, port: Arc<dyn RemoteTransportPort>) -> Self {
        self.ports.insert(kind, Arc::new(move || Arc::clone(&port)));
        self
    }

    /// 已注册的 kind（排序后，便于断言与日志）。
    pub fn registered_kinds(&self) -> Vec<&'static str> {
        let mut kinds: Vec<&'static str> = self.ports.keys().copied().collect();
        kinds.sort_unstable();
        kinds
    }

    pub fn has_kind(&self, kind: &str) -> bool {
        self.ports.contains_key(kind)
    }

    /// 当前被路由的 entry 数。
    pub fn tracked_entries(&self) -> usize {
        self.entries.lock().expect("router entries poisoned").len()
    }

    /// 未注册 kind 的准确错误：说清**是哪种传输没落地**。
    fn unsupported_message(kind: &str) -> String {
        match kind {
            "ssh" => "SSH 远程工作区尚未支持：需要 SSH 客户端（未落地）".to_string(),
            "wsl" => "WSL 远程工作区尚未支持：需要 stdio 客户端（未落地）".to_string(),
            "docker" => "Docker 远程工作区尚未支持：需要容器 exec 客户端（未落地）".to_string(),
            "v3" => "v3 远程工作区尚未支持：需要 Noise 端到端传输（未落地）".to_string(),
            other => format!("远程工作区类型 {other} 尚未支持"),
        }
    }
}

impl RemoteTransportPort for RemoteTransportRouter {
    fn connect(
        &self,
        plan: ConnectPlan,
        target: RemoteTargetSpec,
    ) -> BoxFuture<Result<(), String>> {
        let kind = target.kind();
        let Some(factory) = self.ports.get(kind) else {
            let message = Self::unsupported_message(kind);
            return Box::pin(async move { Err(message) });
        };
        let port = factory();
        let entry_id = plan.entry_id;
        let entries = Arc::clone(&self.entries);
        Box::pin(async move {
            port.connect(plan, target).await?;
            entries
                .lock()
                .expect("router entries poisoned")
                .insert(entry_id, kind);
            Ok(())
        })
    }

    fn channel_client(&self, entry_id: u64) -> Option<Arc<rpc_client::ChannelClient>> {
        let kind = self
            .entries
            .lock()
            .expect("router entries poisoned")
            .get(&entry_id)
            .copied()?;
        self.ports
            .get(kind)
            .and_then(|factory| factory().channel_client(entry_id))
    }

    fn abort(&self, entry_id: u64) {
        let kind = self
            .entries
            .lock()
            .expect("router entries poisoned")
            .remove(&entry_id);
        if let Some(kind) = kind {
            if let Some(factory) = self.ports.get(kind) {
                factory().abort(entry_id);
            }
        }
    }

    fn dispose(&self, entry_id: u64) {
        // 先摘记录：即使端口不存在也不留悬空 entry id。
        let kind = self
            .entries
            .lock()
            .expect("router entries poisoned")
            .remove(&entry_id);
        if let Some(kind) = kind {
            if let Some(factory) = self.ports.get(kind) {
                factory().dispose(entry_id);
            }
        }
    }

    fn release_workspace(
        &self,
        entry_id: u64,
        workspace_key: String,
        generation: u32,
        context: RemoteWorkspaceContext,
    ) -> BoxFuture<Result<(), String>> {
        let kind = self
            .entries
            .lock()
            .expect("router entries poisoned")
            .get(&entry_id)
            .copied();
        let Some(kind) = kind else {
            // 没有登记的 entry：连接从未建立成功，释放无事可做。
            return Box::pin(async { Ok(()) });
        };
        let Some(factory) = self.ports.get(kind) else {
            return Box::pin(async { Ok(()) });
        };
        let port = factory();
        Box::pin(async move {
            port.release_workspace(entry_id, workspace_key, generation, context)
                .await
        })
    }
}
