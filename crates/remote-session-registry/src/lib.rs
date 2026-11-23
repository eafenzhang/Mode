//! 远程 workspace 的 logical session 注册表（16.4d 首切）。
//!
//! 真源：`packages/desktop/src/host/windowRemoteConnectionRegistry.ts`（754 行）。
//! 规范见 `docs/specs/remote-workspaces-channel.md`。
//!
//! 这里只搬**传输无关**的那一层：session 生命周期、按 key 复用连接、generation、
//! 取消语义、最后一个 waiter 取消时的分支。真实建连（LAN WS / SSH stdio / v3 Noise）
//! 由调用方实现，通过 [`RegistryEffect`] 驱动——注册表不持有网络、线程或异步运行时，
//! 因此可以纯状态机测试。
//!
//! 为什么不做成 async：TS 里这些语义全部围绕「谁先完成、迟到结果怎么办」，
//! 用 async 写会把确定性判据埋进调度；把「驱动」与「状态」分开后，
//! 「迟到的成功不能复活已退休的连接」这类不变式可以逐条断言。

use std::collections::{BTreeSet, HashMap};

use workspace_identity::{
    build_remote_workspace_identity, build_ssh_remote_host_key, RemoteTargetRef,
};

/// 建连目标的完整形态（比 `RemoteTargetRef` 多一个 `has_password`）。
///
/// `has_password` 参与 SSH 共享键（authKind），但**密码本身**不进结构——注册表与
/// identity 层都不该看到凭据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteTargetSpec {
    Ssh {
        host: String,
        port: Option<u16>,
        username: String,
        private_key_path: Option<String>,
        has_password: bool,
    },
    Wsl {
        distro: Option<String>,
        user: Option<String>,
    },
    Docker {
        container: String,
    },
    Lan {
        host: String,
        port: u16,
    },
    V3 {
        device_id: String,
    },
}

impl RemoteTargetSpec {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Ssh { .. } => "ssh",
            Self::Wsl { .. } => "wsl",
            Self::Docker { .. } => "docker",
            Self::Lan { .. } => "lan",
            Self::V3 { .. } => "v3",
        }
    }

    /// 投影到 identity 层（secret 与展示字段不参与 identity/判等）。
    pub fn as_identity_target(&self) -> RemoteTargetRef {
        match self {
            Self::Ssh {
                host,
                port,
                username,
                private_key_path,
                ..
            } => RemoteTargetRef::Ssh {
                host: host.clone(),
                port: *port,
                username: username.clone(),
                private_key_path: private_key_path.clone(),
            },
            Self::Wsl { distro, user } => RemoteTargetRef::Wsl {
                distro: distro.clone(),
                user: user.clone(),
            },
            Self::Docker { container } => RemoteTargetRef::Docker {
                container: container.clone(),
            },
            Self::Lan { host, port } => RemoteTargetRef::Lan {
                host: host.clone(),
                port: *port,
            },
            Self::V3 { device_id } => RemoteTargetRef::V3 {
                device_id: device_id.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    Online,
    Closing,
    Failed,
    Disconnected,
}

impl ConnectionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::Online => "online",
            Self::Closing => "closing",
            Self::Failed => "failed",
            Self::Disconnected => "disconnected",
        }
    }
}

/// TS `normalizeWslSegment`：空值归一为 `<default>`（**不是**空串——空串会让
/// `wsl::user` 与 `wsl:<default>:user` 撞键）。
fn normalize_wsl_segment(value: Option<&str>) -> String {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(trimmed) => trimmed.to_string(),
        None => "<default>".to_string(),
    }
}

/// 连接复用键（TS `buildConnectionKey`）。
///
/// 与 TS 逐字节一致是硬要求：键不同会把同一对端当成两个远端，重复建连、重复部署。
/// 注意 `docker` **不按 target 复用**（用 sessionId 做 dedicated 键）；`wsl` 用 `\0`
/// 分隔 distro 与 user，避免 `a` + `b:c` 这类组合歧义。
pub fn build_connection_key(target: &RemoteTargetSpec, remote_session_id: &str) -> String {
    match target {
        RemoteTargetSpec::Ssh {
            host,
            port,
            username,
            private_key_path,
            has_password,
        } => format!(
            "ssh:{}",
            build_ssh_remote_host_key(
                host,
                *port,
                username,
                private_key_path.as_deref(),
                *has_password
            )
        ),
        RemoteTargetSpec::Wsl { distro, user } => format!(
            "wsl:{}\0{}",
            normalize_wsl_segment(distro.as_deref()),
            normalize_wsl_segment(user.as_deref())
        ),
        // Docker 保持 dedicated 生命周期：每次 connect 都是独立通道。
        RemoteTargetSpec::Docker { .. } => format!("docker:dedicated:{remote_session_id}"),
        RemoteTargetSpec::Lan { host, port } => {
            format!("lan:{}:{port}", host.trim().to_lowercase())
        }
        RemoteTargetSpec::V3 { device_id } => format!("v3:{device_id}"),
    }
}

/// session 快照（`IRemoteService` 的 `getSession`/`listSessions` 返回值）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSessionSnapshot {
    pub remote_session_id: String,
    pub request_id: String,
    pub target: RemoteTargetSpec,
    pub workspace_path: Option<String>,
    pub workspace_identity: Option<String>,
    /// connect 时为 1，每次 `bind_context` 加 1；renderer 用它判断快照新鲜度。
    pub generation: u32,
    pub state: ConnectionState,
    pub source_availability: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectRequest {
    pub request_id: String,
    pub target: RemoteTargetSpec,
    pub workspace_path: Option<String>,
    pub workspace_identity: Option<String>,
}

/// `connect` 的结果：告诉调用方「复用已有 entry」还是「新建 entry 并起传输」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectPlan {
    pub remote_session_id: String,
    pub entry_id: u64,
    /// `true` = 复用已 online 的 entry（无需再建连，直接可返回）；`false` = 需要新传输。
    pub reused_online_entry: bool,
    /// 需要新建传输时给出连接键（供调用方按自己的方式建连）。
    pub connection_key: String,
}

/// 注册表要求调用方执行的动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryEffect {
    /// 起传输（新建 entry 时）。
    Connect {
        entry_id: u64,
        connection_key: String,
        target: RemoteTargetSpec,
    },
    /// 中止传输（entry 已退休；迟到的完成必须被丢弃）。
    AbortTransport { entry_id: u64 },
    /// 释放 workspace 运行时归属。
    ReleaseWorkspace {
        remote_session_id: String,
        workspace_key: String,
    },
    /// 排 idle 销毁（WSL online 且最后一个 waiter 取消）。
    ScheduleIdleDispose { entry_id: u64 },
    /// 立即销毁（其余 kind 的最后一个 waiter 取消）。
    DisposeEntry { entry_id: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    /// `requestId` 重复（TS 抛错，不静默复用——重复的 requestId 意味着调用方状态错乱）。
    DuplicateRequestId(String),
    /// `remoteSessionId` 不存在。
    UnknownSession(String),
    /// 注册表已释放。
    Disposed,
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateRequestId(id) => {
                write!(formatter, "远程连接 requestId 重复，requestId={id}")
            }
            Self::UnknownSession(id) => {
                write!(
                    formatter,
                    "未找到远程 logical session，remoteSessionId={id}"
                )
            }
            Self::Disposed => write!(formatter, "窗口 Host 的远程连接 registry 已释放"),
        }
    }
}

impl std::error::Error for RegistryError {}

#[derive(Debug, Clone)]
struct Entry {
    key: String,
    target: RemoteTargetSpec,
    state: ConnectionState,
    disposed: bool,
    aborted: bool,
    sessions: BTreeSet<String>,
}

#[derive(Debug, Clone)]
struct Session {
    remote_session_id: String,
    request_id: String,
    target: RemoteTargetSpec,
    workspace_path: Option<String>,
    workspace_identity: Option<String>,
    generation: u32,
    state: ConnectionState,
    source_availability: &'static str,
    entry_id: u64,
    workspace_key: Option<String>,
    cancelled: bool,
}

impl Session {
    fn snapshot(&self) -> RemoteSessionSnapshot {
        RemoteSessionSnapshot {
            remote_session_id: self.remote_session_id.clone(),
            request_id: self.request_id.clone(),
            target: self.target.clone(),
            workspace_path: self.workspace_path.clone(),
            workspace_identity: self.workspace_identity.clone(),
            generation: self.generation,
            state: self.state,
            source_availability: self.source_availability,
        }
    }
}

#[derive(Default)]
pub struct RemoteSessionRegistry {
    next_entry_id: u64,
    entries: HashMap<u64, Entry>,
    /// key → entry id（连接复用表）。退休时按**对象身份**删除，不只是按 key。
    entry_by_key: HashMap<String, u64>,
    sessions: HashMap<String, Session>,
    session_by_request_id: HashMap<String, String>,
    disposed: bool,
}

impl RemoteSessionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// TS `connect` 的前半段：校验 + 决策（复用还是新建）。
    ///
    /// 返回 [`ConnectPlan`] 与需要执行的动作；调用方随后用
    /// [`Self::complete_connect`] / [`Self::fail_connect`] 交付传输结果。
    pub fn begin_connect(
        &mut self,
        request: ConnectRequest,
    ) -> Result<(ConnectPlan, Vec<RegistryEffect>), RegistryError> {
        if self.disposed {
            return Err(RegistryError::Disposed);
        }
        if self.session_by_request_id.contains_key(&request.request_id) {
            return Err(RegistryError::DuplicateRequestId(request.request_id));
        }

        let remote_session_id = format!("sess-{}", self.next_entry_id + 1);
        let entry_id = if let Some(existing_id) =
            self.reusable_entry_id(&request.target, &remote_session_id)
        {
            existing_id
        } else {
            self.create_entry(&request.target, &remote_session_id)
        };

        let workspace_key = request
            .workspace_identity
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| request.workspace_path.clone());

        let entry_state = self
            .entries
            .get(&entry_id)
            .map(|entry| entry.state)
            .unwrap_or(ConnectionState::Connecting);
        let online = entry_state == ConnectionState::Online;

        self.sessions.insert(
            remote_session_id.clone(),
            Session {
                remote_session_id: remote_session_id.clone(),
                request_id: request.request_id.clone(),
                target: request.target.clone(),
                workspace_path: request.workspace_path,
                workspace_identity: request.workspace_identity,
                generation: 1,
                state: if online {
                    ConnectionState::Online
                } else {
                    ConnectionState::Connecting
                },
                source_availability: if online { "online" } else { "offline" },
                entry_id,
                workspace_key,
                cancelled: false,
            },
        );
        self.session_by_request_id
            .insert(request.request_id, remote_session_id.clone());
        if let Some(entry) = self.entries.get_mut(&entry_id) {
            entry.sessions.insert(remote_session_id.clone());
        }

        let connection_key = self
            .entries
            .get(&entry_id)
            .map(|entry| entry.key.clone())
            .unwrap_or_default();
        let plan = ConnectPlan {
            remote_session_id,
            entry_id,
            reused_online_entry: online,
            connection_key: connection_key.clone(),
        };
        let effects = if online {
            // 复用已就绪的连接：不需要再起传输。
            Vec::new()
        } else {
            // connecting 阶段复用 entry 时也不重复起传输（TS 的 ready promise 共享）。
            let already_connecting = self
                .entries
                .get(&entry_id)
                .map(|entry| entry.sessions.len() > 1)
                .unwrap_or(false);
            if already_connecting {
                Vec::new()
            } else {
                vec![RegistryEffect::Connect {
                    entry_id,
                    connection_key,
                    target: request.target,
                }]
            }
        };
        Ok((plan, effects))
    }

    /// TS `connect` 的后半段：传输成功。
    ///
    /// 「迟到完成」守卫：entry 已 disposed/aborted、或已没有 session 时，
    /// **不复活**，直接返回 `DisposeEntry`（TS 里是立即 `disposeEntry(entry)`）。
    pub fn complete_connect(&mut self, entry_id: u64) -> Vec<RegistryEffect> {
        let Some(entry) = self.entries.get(&entry_id) else {
            return Vec::new();
        };
        let orphaned = entry.disposed || entry.aborted || entry.sessions.is_empty();
        if orphaned {
            if let Some(entry) = self.entries.get_mut(&entry_id) {
                entry.state = ConnectionState::Closing;
                entry.disposed = true;
            }
            self.retire_entry_key(entry_id);
            return vec![RegistryEffect::DisposeEntry { entry_id }];
        }

        if let Some(entry) = self.entries.get_mut(&entry_id) {
            entry.state = ConnectionState::Online;
        }
        let session_ids: Vec<String> = self
            .entries
            .get(&entry_id)
            .map(|entry| entry.sessions.iter().cloned().collect())
            .unwrap_or_default();
        for session_id in session_ids {
            if let Some(session) = self.sessions.get_mut(&session_id) {
                session.state = ConnectionState::Online;
                session.source_availability = "online";
            }
        }
        Vec::new()
    }

    /// 传输失败：entry 与它的所有 session 一起失败并移除（TS 同）。
    pub fn fail_connect(&mut self, entry_id: u64) -> Vec<RegistryEffect> {
        let Some(entry) = self.entries.remove(&entry_id) else {
            return Vec::new();
        };
        if self.entry_by_key.get(&entry.key) == Some(&entry_id) {
            self.entry_by_key.remove(&entry.key);
        }
        let mut effects = Vec::new();
        for session_id in entry.sessions {
            if let Some(session) = self.sessions.remove(&session_id) {
                self.session_by_request_id.remove(&session.request_id);
                if let Some(workspace_key) = session.workspace_key.clone() {
                    effects.push(RegistryEffect::ReleaseWorkspace {
                        remote_session_id: session_id,
                        workspace_key,
                    });
                }
            }
        }
        effects
    }

    /// TS `cancelConnect`：幂等；未找到或已取消都直接返回。
    ///
    /// 最后一个 waiter 取消时按 kind 分支（见 spec §3.3），这里是该分支的唯一实现点。
    pub fn cancel_connect(&mut self, request_id: &str) -> Vec<RegistryEffect> {
        let Some(session_id) = self.session_by_request_id.get(request_id).cloned() else {
            return Vec::new();
        };
        let Some(session) = self.sessions.get(&session_id) else {
            return Vec::new();
        };
        if session.cancelled {
            return Vec::new();
        }
        let entry_id = session.entry_id;
        let workspace_key = session.workspace_key.clone();
        if let Some(session) = self.sessions.get_mut(&session_id) {
            session.cancelled = true;
        }
        self.session_by_request_id.remove(request_id);
        self.sessions.remove(&session_id);

        let mut effects = Vec::new();
        if let Some(workspace_key) = workspace_key {
            effects.push(RegistryEffect::ReleaseWorkspace {
                remote_session_id: session_id.clone(),
                workspace_key,
            });
        }

        let Some(entry) = self.entries.get_mut(&entry_id) else {
            return effects;
        };
        entry.sessions.remove(&session_id);
        if !entry.sessions.is_empty() {
            return effects;
        }

        // 最后一个 waiter 走了。
        match (entry.target.kind(), entry.state) {
            ("ssh", ConnectionState::Connecting) => {
                // 旧 entry 仍以 connecting 留在复用表；立即重连会继续等待已经 aborted 的
                // readiness 并沿用上一次凭据。按对象身份退休，再触发底层取消——迟到的
                // 旧完成不能删除或复活同 key 的新连接。
                if self.entry_by_key.get(&entry.key) == Some(&entry_id) {
                    self.entry_by_key.remove(&entry.key);
                }
                if let Some(entry) = self.entries.get_mut(&entry_id) {
                    entry.state = ConnectionState::Closing;
                    entry.aborted = true;
                }
                effects.push(RegistryEffect::AbortTransport { entry_id });
            }
            ("wsl", ConnectionState::Online) => {
                effects.push(RegistryEffect::ScheduleIdleDispose { entry_id });
            }
            ("ssh", ConnectionState::Online) => {
                // ready 的 SSH 连接是窗口 cache；取消一次 logical attach 不该把
                // 后续 workspace 的复用连接一并销毁。真实关窗时统一释放。
            }
            _ => {
                effects.push(RegistryEffect::DisposeEntry { entry_id });
            }
        }
        effects
    }

    /// TS `bindContext`：换绑 workspace，`generation` 加 1。
    pub fn bind_context(
        &mut self,
        remote_session_id: &str,
        workspace_path: &str,
        workspace_identity: &str,
    ) -> Result<Vec<RegistryEffect>, RegistryError> {
        let Some(session) = self.sessions.get_mut(remote_session_id) else {
            return Err(RegistryError::UnknownSession(remote_session_id.to_string()));
        };
        let previous_key = session.workspace_key.clone();
        session.workspace_path = Some(workspace_path.to_string());
        session.workspace_identity = Some(workspace_identity.to_string());
        session.generation += 1;
        let next_key = {
            let identity = workspace_identity.trim();
            if identity.is_empty() {
                workspace_path.to_string()
            } else {
                identity.to_string()
            }
        };
        session.workspace_key = Some(next_key.clone());

        let mut effects = Vec::new();
        if previous_key.as_deref() != Some(next_key.as_str()) {
            if let Some(previous) = previous_key {
                effects.push(RegistryEffect::ReleaseWorkspace {
                    remote_session_id: remote_session_id.to_string(),
                    workspace_key: previous,
                });
            }
        }
        Ok(effects)
    }

    /// TS `dispose`：释放一个 logical session 及其 workspace 归属。
    pub fn dispose_session(&mut self, remote_session_id: &str) -> Vec<RegistryEffect> {
        let Some(session) = self.sessions.remove(remote_session_id) else {
            return Vec::new();
        };
        self.session_by_request_id.remove(&session.request_id);
        let mut effects = Vec::new();
        if let Some(workspace_key) = session.workspace_key {
            effects.push(RegistryEffect::ReleaseWorkspace {
                remote_session_id: remote_session_id.to_string(),
                workspace_key,
            });
        }
        if let Some(entry) = self.entries.get_mut(&session.entry_id) {
            entry.sessions.remove(remote_session_id);
            if entry.sessions.is_empty() {
                match (entry.target.kind(), entry.state) {
                    ("ssh", ConnectionState::Online) | ("wsl", ConnectionState::Online) => {
                        if entry.target.kind() == "wsl" {
                            effects.push(RegistryEffect::ScheduleIdleDispose {
                                entry_id: session.entry_id,
                            });
                        }
                    }
                    _ => effects.push(RegistryEffect::DisposeEntry {
                        entry_id: session.entry_id,
                    }),
                }
            }
        }
        effects
    }

    /// TS `getSession`。
    pub fn get_session(&self, remote_session_id: &str) -> Option<RemoteSessionSnapshot> {
        self.sessions.get(remote_session_id).map(Session::snapshot)
    }

    /// TS `listSessions`。
    pub fn list_sessions(&self) -> Vec<RemoteSessionSnapshot> {
        let mut sessions: Vec<RemoteSessionSnapshot> =
            self.sessions.values().map(Session::snapshot).collect();
        sessions.sort_by(|left, right| left.remote_session_id.cmp(&right.remote_session_id));
        sessions
    }

    /// 连接被底层关闭（TS `handleConnectionClosed`）：所有 session 转 disconnected/offline。
    pub fn handle_entry_closed(&mut self, entry_id: u64) -> Vec<RegistryEffect> {
        let Some(entry) = self.entries.get_mut(&entry_id) else {
            return Vec::new();
        };
        if entry.disposed || entry.state == ConnectionState::Closing {
            return Vec::new();
        }
        entry.state = ConnectionState::Disconnected;
        let session_ids: Vec<String> = entry.sessions.iter().cloned().collect();
        for session_id in &session_ids {
            if let Some(session) = self.sessions.get_mut(session_id) {
                session.state = ConnectionState::Disconnected;
                session.source_availability = "offline";
            }
        }
        self.retire_entry_key(entry_id);
        Vec::new()
    }

    /// 释放整个注册表（TS `disposeHost`）。
    pub fn dispose(&mut self) -> Vec<RegistryEffect> {
        if self.disposed {
            return Vec::new();
        }
        self.disposed = true;
        let entry_ids: Vec<u64> = self.entries.keys().copied().collect();
        self.sessions.clear();
        self.session_by_request_id.clear();
        self.entry_by_key.clear();
        self.entries.clear();
        entry_ids
            .into_iter()
            .map(|entry_id| RegistryEffect::DisposeEntry { entry_id })
            .collect()
    }

    fn reusable_entry_id(&self, target: &RemoteTargetSpec, remote_session_id: &str) -> Option<u64> {
        let key = build_connection_key(target, remote_session_id);
        let entry_id = *self.entry_by_key.get(&key)?;
        let entry = self.entries.get(&entry_id)?;
        let reusable = !entry.disposed
            && !entry.aborted
            && matches!(
                entry.state,
                ConnectionState::Connecting | ConnectionState::Online
            );
        reusable.then_some(entry_id)
    }

    fn create_entry(&mut self, target: &RemoteTargetSpec, remote_session_id: &str) -> u64 {
        self.next_entry_id += 1;
        let entry_id = self.next_entry_id;
        let key = build_connection_key(target, remote_session_id);
        self.entry_by_key.insert(key.clone(), entry_id);
        self.entries.insert(
            entry_id,
            Entry {
                key,
                target: target.clone(),
                state: ConnectionState::Connecting,
                disposed: false,
                aborted: false,
                sessions: BTreeSet::new(),
            },
        );
        entry_id
    }

    fn retire_entry_key(&mut self, entry_id: u64) {
        if let Some(entry) = self.entries.get(&entry_id) {
            if self.entry_by_key.get(&entry.key) == Some(&entry_id) {
                self.entry_by_key.remove(&entry.key);
            }
        }
    }
}

/// 便捷：按 target 与路径派生 workspace identity（供 `connect` 的缺省 identity 用）。
pub fn derive_workspace_identity(workspace_path: &str, target: &RemoteTargetSpec) -> String {
    build_remote_workspace_identity(workspace_path, &target.as_identity_target())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lan(host: &str, port: u16) -> RemoteTargetSpec {
        RemoteTargetSpec::Lan {
            host: host.to_string(),
            port,
        }
    }

    fn request(request_id: &str, target: RemoteTargetSpec, path: Option<&str>) -> ConnectRequest {
        ConnectRequest {
            request_id: request_id.to_string(),
            target,
            workspace_path: path.map(str::to_string),
            workspace_identity: None,
        }
    }

    #[test]
    fn connection_keys_match_the_ts_shapes() {
        assert_eq!(
            build_connection_key(&lan(" 10.0.0.5 ", 45879), "s1"),
            "lan:10.0.0.5:45879"
        );
        assert_eq!(
            build_connection_key(
                &RemoteTargetSpec::Wsl {
                    distro: None,
                    user: Some("  ".to_string()),
                },
                "s1"
            ),
            "wsl:<default>\0<default>",
            "空 distro/user 必须归一为 <default>，否则会与显式同名值撞键"
        );
        assert_eq!(
            build_connection_key(
                &RemoteTargetSpec::Wsl {
                    distro: Some(" Ubuntu ".to_string()),
                    user: Some("dev".to_string()),
                },
                "s1"
            ),
            "wsl:Ubuntu\0dev"
        );
        // docker 刻意不按 target 复用：同一 container 两次 connect 是两个通道。
        let docker = RemoteTargetSpec::Docker {
            container: "box".to_string(),
        };
        assert_eq!(build_connection_key(&docker, "s1"), "docker:dedicated:s1");
        assert_eq!(build_connection_key(&docker, "s2"), "docker:dedicated:s2");
        assert_eq!(
            build_connection_key(
                &RemoteTargetSpec::V3 {
                    device_id: "dev-1".to_string()
                },
                "s1"
            ),
            "v3:dev-1"
        );
    }

    #[test]
    fn duplicate_request_id_is_rejected() {
        let mut registry = RemoteSessionRegistry::new();
        registry
            .begin_connect(request("req-1", lan("h", 1), Some("/w")))
            .expect("first");
        let error = registry
            .begin_connect(request("req-1", lan("h", 1), Some("/w")))
            .expect_err("duplicate");
        assert_eq!(
            error,
            RegistryError::DuplicateRequestId("req-1".to_string())
        );
    }

    #[test]
    fn same_lan_target_reuses_one_entry_but_keeps_separate_sessions() {
        let mut registry = RemoteSessionRegistry::new();
        let (first, first_effects) = registry
            .begin_connect(request("req-1", lan("h", 1), Some("/a")))
            .expect("first");
        assert!(matches!(
            first_effects.as_slice(),
            [RegistryEffect::Connect { .. }]
        ));

        let (second, second_effects) = registry
            .begin_connect(request("req-2", lan("h", 1), Some("/b")))
            .expect("second");
        // 同一对端复用同一条通道；第二个 session 不该再起一次传输。
        assert_eq!(first.entry_id, second.entry_id);
        assert!(second_effects.is_empty(), "复用 entry 不应重复建连");
        assert_ne!(first.remote_session_id, second.remote_session_id);
        assert_eq!(registry.list_sessions().len(), 2);
    }

    #[test]
    fn docker_never_reuses_entries() {
        let mut registry = RemoteSessionRegistry::new();
        let docker = RemoteTargetSpec::Docker {
            container: "box".to_string(),
        };
        let (first, _) = registry
            .begin_connect(request("req-1", docker.clone(), Some("/a")))
            .expect("first");
        let (second, effects) = registry
            .begin_connect(request("req-2", docker, Some("/a")))
            .expect("second");
        assert_ne!(first.entry_id, second.entry_id);
        assert_eq!(effects.len(), 1, "每个 docker session 都要自己的通道");
    }

    #[test]
    fn cancel_is_idempotent_and_retires_ssh_connecting_entry() {
        let mut registry = RemoteSessionRegistry::new();
        let ssh = RemoteTargetSpec::Ssh {
            host: "h".to_string(),
            port: None,
            username: "u".to_string(),
            private_key_path: None,
            has_password: false,
        };
        let (plan, _) = registry
            .begin_connect(request("req-1", ssh.clone(), Some("/w")))
            .expect("connect");
        let effects = registry.cancel_connect("req-1");
        assert!(
            effects.contains(&RegistryEffect::AbortTransport {
                entry_id: plan.entry_id
            }),
            "ssh 处于 connecting 时必须按对象身份退休并 abort：{effects:?}"
        );
        // 幂等：再取消一次无副作用。
        assert!(registry.cancel_connect("req-1").is_empty());
        assert!(registry.get_session(&plan.remote_session_id).is_none());

        // 重连必须拿到**新** entry（旧 entry 已退休，不能等一个已 abort 的 readiness）。
        let (next, _) = registry
            .begin_connect(request("req-2", ssh, Some("/w")))
            .expect("reconnect");
        assert_ne!(next.entry_id, plan.entry_id);
    }

    #[test]
    fn cancel_keeps_online_ssh_entry_but_idles_online_wsl() {
        let mut registry = RemoteSessionRegistry::new();
        let ssh = RemoteTargetSpec::Ssh {
            host: "h".to_string(),
            port: None,
            username: "u".to_string(),
            private_key_path: None,
            has_password: false,
        };
        let (ssh_plan, _) = registry
            .begin_connect(request("req-1", ssh, Some("/w")))
            .expect("ssh connect");
        registry.complete_connect(ssh_plan.entry_id);
        let ssh_effects = registry.cancel_connect("req-1");
        // 只释放 workspace 归属；**不得**出现 DisposeEntry/AbortTransport——
        // online 的 SSH entry 是窗口 cache，取消一次 attach 不该销毁它。
        assert!(
            ssh_effects
                .iter()
                .all(|effect| matches!(effect, RegistryEffect::ReleaseWorkspace { .. })),
            "online 的 SSH entry 不该被销毁：{ssh_effects:?}"
        );
        // entry 仍在复用表里：同 key 再连必须复用（不是新建）。
        let (again, _) = registry
            .begin_connect(request(
                "req-9",
                RemoteTargetSpec::Ssh {
                    host: "h".to_string(),
                    port: None,
                    username: "u".to_string(),
                    private_key_path: None,
                    has_password: false,
                },
                Some("/w"),
            ))
            .expect("reuse");
        assert_eq!(again.entry_id, ssh_plan.entry_id, "ssh cache 应被复用");

        let mut registry = RemoteSessionRegistry::new();
        let wsl = RemoteTargetSpec::Wsl {
            distro: Some("Ubuntu".to_string()),
            user: None,
        };
        let (wsl_plan, _) = registry
            .begin_connect(request("req-2", wsl, Some("/w")))
            .expect("wsl connect");
        registry.complete_connect(wsl_plan.entry_id);
        let wsl_plan_effects = registry.cancel_connect("req-2");
        // 只保留 idle 销毁，不得立即 DisposeEntry。
        assert!(
            wsl_plan_effects
                .iter()
                .any(|effect| matches!(effect, RegistryEffect::ScheduleIdleDispose { .. })),
            "online 的 WSL 应走 idle 销毁：{wsl_plan_effects:?}"
        );
        assert!(
            wsl_plan_effects
                .iter()
                .all(|effect| !matches!(effect, RegistryEffect::DisposeEntry { .. })),
            "online 的 WSL 不该立刻销毁（启动贵）：{wsl_plan_effects:?}"
        );
    }

    #[test]
    fn late_completion_never_revives_a_retired_entry() {
        let mut registry = RemoteSessionRegistry::new();
        let ssh = RemoteTargetSpec::Ssh {
            host: "h".to_string(),
            port: None,
            username: "u".to_string(),
            private_key_path: None,
            has_password: false,
        };
        let (plan, _) = registry
            .begin_connect(request("req-1", ssh, Some("/w")))
            .expect("connect");
        registry.cancel_connect("req-1");

        // 传输迟到的成功结果：必须立即销毁，不能把 session 拉回 online。
        assert_eq!(
            registry.complete_connect(plan.entry_id),
            vec![RegistryEffect::DisposeEntry {
                entry_id: plan.entry_id
            }]
        );
        assert!(registry.get_session(&plan.remote_session_id).is_none());
        assert!(registry.list_sessions().is_empty());
    }

    #[test]
    fn bind_context_bumps_generation_and_releases_the_old_workspace_only_on_change() {
        let mut registry = RemoteSessionRegistry::new();
        let (plan, _) = registry
            .begin_connect(request("req-1", lan("h", 1), Some("/old")))
            .expect("connect");
        registry.complete_connect(plan.entry_id);
        assert_eq!(
            registry
                .get_session(&plan.remote_session_id)
                .expect("session")
                .generation,
            1
        );

        // 换到新 workspace：generation +1，且释放旧归属。
        let effects = registry
            .bind_context(&plan.remote_session_id, "/new", "remote:lan:h:1:/new")
            .expect("bind");
        assert_eq!(
            registry
                .get_session(&plan.remote_session_id)
                .expect("session")
                .generation,
            2
        );
        assert_eq!(
            effects,
            vec![RegistryEffect::ReleaseWorkspace {
                remote_session_id: plan.remote_session_id.clone(),
                workspace_key: "/old".to_string(),
            }]
        );

        // 同一 workspace 再绑：generation 仍要 +1（每次绑都是新的一代），但不释放。
        let effects = registry
            .bind_context(&plan.remote_session_id, "/new", "remote:lan:h:1:/new")
            .expect("rebind");
        assert_eq!(
            registry
                .get_session(&plan.remote_session_id)
                .expect("session")
                .generation,
            3
        );
        assert!(effects.is_empty());

        assert_eq!(
            registry.bind_context("missing", "/x", "y"),
            Err(RegistryError::UnknownSession("missing".to_string()))
        );
    }

    #[test]
    fn identity_wins_over_path_in_the_workspace_key() {
        let mut registry = RemoteSessionRegistry::new();
        let (plan, _) = registry
            .begin_connect(ConnectRequest {
                request_id: "req-1".to_string(),
                target: lan("h", 1),
                workspace_path: Some("/w".to_string()),
                workspace_identity: Some("  remote:lan:h:1:/w  ".to_string()),
            })
            .expect("connect");
        let effects = registry
            .bind_context(&plan.remote_session_id, "/w2", " ")
            .expect("bind");
        // identity 为空时退回 path；旧 key 是 trim 过的 identity。
        assert_eq!(
            effects,
            vec![RegistryEffect::ReleaseWorkspace {
                remote_session_id: plan.remote_session_id.clone(),
                workspace_key: "remote:lan:h:1:/w".to_string(),
            }]
        );
    }

    #[test]
    fn failure_removes_entry_and_sessions() {
        let mut registry = RemoteSessionRegistry::new();
        let (plan, _) = registry
            .begin_connect(request("req-1", lan("h", 1), Some("/w")))
            .expect("connect");
        let effects = registry.fail_connect(plan.entry_id);
        assert!(matches!(
            effects.as_slice(),
            [RegistryEffect::ReleaseWorkspace { .. }]
        ));
        assert!(registry.list_sessions().is_empty());
        // entry 已移除：同 key 再连必须新建。
        let (next, _) = registry
            .begin_connect(request("req-2", lan("h", 1), Some("/w")))
            .expect("retry");
        assert_ne!(next.entry_id, plan.entry_id);
    }

    #[test]
    fn closing_an_entry_marks_sessions_offline_and_retires_the_key() {
        let mut registry = RemoteSessionRegistry::new();
        let (plan, _) = registry
            .begin_connect(request("req-1", lan("h", 1), Some("/w")))
            .expect("connect");
        registry.complete_connect(plan.entry_id);
        registry.handle_entry_closed(plan.entry_id);

        let snapshot = registry
            .get_session(&plan.remote_session_id)
            .expect("session");
        assert_eq!(snapshot.state, ConnectionState::Disconnected);
        assert_eq!(snapshot.source_availability, "offline");

        let (next, _) = registry
            .begin_connect(request("req-2", lan("h", 1), Some("/w")))
            .expect("reconnect");
        assert_ne!(next.entry_id, plan.entry_id, "已断开的 entry 不能复用");
    }

    #[test]
    fn dispose_releases_everything_and_rejects_new_connects() {
        let mut registry = RemoteSessionRegistry::new();
        let (plan, _) = registry
            .begin_connect(request("req-1", lan("h", 1), Some("/w")))
            .expect("connect");
        let effects = registry.dispose();
        assert_eq!(
            effects,
            vec![RegistryEffect::DisposeEntry {
                entry_id: plan.entry_id
            }]
        );
        assert!(registry.list_sessions().is_empty());
        assert_eq!(
            registry
                .begin_connect(request("req-2", lan("h", 1), Some("/w")))
                .expect_err("disposed"),
            RegistryError::Disposed
        );
    }

    #[test]
    fn derive_identity_uses_the_shared_builder() {
        assert_eq!(
            derive_workspace_identity("/w", &lan("H", 1)),
            "remote:lan:h:1:/w"
        );
    }
}
