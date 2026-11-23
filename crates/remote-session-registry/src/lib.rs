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
        /// 已配对对端的稳定 id：端口据此从凭据里取令牌与端点列表。
        server_id: Option<String>,
        /// 连接期透明携带的一次性令牌（TS `LanConnectOptions.token` 同语义）。
        ///
        /// **不进 identity、不进连接键、不进任何下发描述符**——落盘一律走凭据服务。
        token: Option<String>,
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
            Self::Lan { host, port, .. } => RemoteTargetRef::Lan {
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
        RemoteTargetSpec::Lan { host, port, .. } => {
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

/// workspace 运行时上下文（TS `RemoteWorkspaceContext`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteWorkspaceContext {
    pub workspace_path: String,
    pub workspace_identity: Option<String>,
}

/// workspace 运行时就绪态（TS `workspaceReadyState`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceReadyState {
    Pending,
    Ready,
    Failed,
}

/// 一个 entry 内某个 workspace 的运行时代际（TS `WorkspaceRuntimeState`）。
///
/// `generation` 是**归属代际**而不是连接代际：只要上一代 release 还没完成、
/// 新 owner 又抢到了同一个 workspace，代际就要 +1，让尚未开始的旧 release 失效。
#[derive(Debug, Clone)]
struct WorkspaceRuntimeState {
    context: RemoteWorkspaceContext,
    generation: u32,
    /// 因 running task > 0 而推迟的释放代际（task 归零后要补做）。
    pending_release_generation: Option<u32>,
    /// 是否有一次 release 正在进行（TS 的 `releaseInFlight` promise）。
    release_in_flight: bool,
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
    /// 释放 workspace 运行时（**仅 WSL**）。
    ///
    /// 调用方完成后必须回传 [`RemoteSessionRegistry::complete_workspace_release`]，
    /// 否则同 workspace 的下一代 owner 会永远停在 `Pending`。
    ReleaseWorkspace {
        entry_id: u64,
        workspace_key: String,
        generation: u32,
        context: RemoteWorkspaceContext,
    },
    /// 清掉该 entry 的 idle 定时器（有 task 在跑时不能 idle 销毁）。
    ClearIdleTimer { entry_id: u64 },
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
    /// 同一 workspace 匹配到多个 logical session（上层绑定逻辑出错，不能静默挑一个）。
    AmbiguousWorkspace(String),
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
            Self::AmbiguousWorkspace(path) => write!(
                formatter,
                "远程 workspace scope 匹配到多个 logical session，workspacePath={path}"
            ),
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
    /// 该 entry 上被持有过的 workspace（`setWorkspaceRunningTaskCount` 只认这些键）。
    workspace_keys: BTreeSet<String>,
    /// workspaceKey → 运行时代际。
    workspace_runtime: HashMap<String, WorkspaceRuntimeState>,
    /// workspaceKey → 正在跑的 task 数（>0 时推迟 release、禁 idle 销毁）。
    running_task_count: HashMap<String, u32>,
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
    /// 该 session 持有的 workspace 代际（release 时用来判断「是不是我这一代」）。
    workspace_generation: Option<u32>,
    workspace_ready: WorkspaceReadyState,
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
    /// session id 用独立计数器：复用 entry 时 entry 计数器不动，共用会让后续
    /// connect 撞出重复的 `sess-*`。
    next_session_id: u64,
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
    ///
    /// WSL 目标会在这一步同步完成 workspace 运行时的归属登记（TS 同）：
    /// 非 WSL 直接 `Ready`；WSL 若上一代 release 还在飞，则停在 `Pending`，
    /// 调用方必须在 release 完成后回传 `complete_workspace_release`。
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

        // session 与 entry 各有独立计数器：复用 entry 时 entry 计数器不动，
        // 若复用同一个计数器生成 sessionId，第三次 connect 就会撞出重复 id。
        self.next_session_id += 1;
        let remote_session_id = format!("sess-{}", self.next_session_id);
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
                workspace_path: request.workspace_path.clone(),
                workspace_identity: request.workspace_identity.clone(),
                generation: 1,
                state: if online {
                    ConnectionState::Online
                } else {
                    ConnectionState::Connecting
                },
                source_availability: if online { "online" } else { "offline" },
                entry_id,
                workspace_key: workspace_key.clone(),
                // generation 在 prepare 里赋值；先置空以便区分「尚未登记」。
                workspace_generation: None,
                workspace_ready: if online {
                    WorkspaceReadyState::Ready
                } else {
                    WorkspaceReadyState::Pending
                },
                cancelled: false,
            },
        );
        let duplicate_session = self
            .session_by_request_id
            .insert(request.request_id, remote_session_id.clone());
        debug_assert!(duplicate_session.is_none(), "requestId 已在前面查重");
        if let Some(entry) = self.entries.get_mut(&entry_id) {
            entry.sessions.insert(remote_session_id.clone());
            if let Some(key) = workspace_key.as_ref() {
                entry.workspace_keys.insert(key.clone());
            }
        }

        // workspace 运行时归属（仅 WSL 有意义；其余直接 Ready）。
        let mut effects = self.prepare_workspace_runtime(&remote_session_id);

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
        if !online {
            // connecting 阶段复用 entry 时也不重复起传输（TS 的 ready promise 共享）。
            let shared_connecting = self
                .entries
                .get(&entry_id)
                .map(|entry| entry.sessions.len() > 1)
                .unwrap_or(false);
            if !shared_connecting {
                effects.insert(
                    0,
                    RegistryEffect::Connect {
                        entry_id,
                        connection_key,
                        target: request.target,
                    },
                );
            }
        }
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
        for session_id in entry.sessions {
            if let Some(session) = self.sessions.remove(&session_id) {
                self.session_by_request_id.remove(&session.request_id);
            }
        }
        Vec::new()
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
        let workspace_generation = session.workspace_generation;
        if let Some(session) = self.sessions.get_mut(&session_id) {
            session.cancelled = true;
        }
        self.session_by_request_id.remove(request_id);
        self.sessions.remove(&session_id);
        if let Some(entry) = self.entries.get_mut(&entry_id) {
            entry.sessions.remove(&session_id);
        }

        // 先登记 workspace 释放（owner 已从 sessions 移除，判据才成立）。
        let mut effects = self.request_workspace_release(
            entry_id,
            &session_id,
            workspace_key.as_deref(),
            workspace_generation,
        );

        let Some(entry) = self.entries.get(&entry_id) else {
            return effects;
        };
        if !entry.sessions.is_empty() {
            return effects;
        }
        let kind = entry.target.kind();
        let state = entry.state;

        // 最后一个 waiter 走了。
        match (kind, state) {
            ("ssh", ConnectionState::Connecting) => {
                // 旧 entry 仍以 connecting 留在复用表；立即重连会继续等待已经 aborted 的
                // readiness 并沿用上一次凭据。按对象身份退休，再触发底层取消——迟到的
                // 旧完成不能删除或复活同 key 的新连接。
                self.retire_entry_key(entry_id);
                if let Some(entry) = self.entries.get_mut(&entry_id) {
                    entry.state = ConnectionState::Closing;
                    entry.aborted = true;
                }
                effects.push(RegistryEffect::AbortTransport { entry_id });
            }
            ("wsl", ConnectionState::Online) => {
                if let Some(effects_idle) =
                    self.schedule_idle_dispose(entry_id, "cancelled-logical-connect")
                {
                    effects.extend(effects_idle);
                }
            }
            ("ssh", ConnectionState::Online) => {
                // ready 的 SSH 连接是窗口 cache；取消一次 logical attach 不该把
                // 后续 workspace 的复用连接一并销毁。真实关窗时统一释放。
                effects.push(RegistryEffect::ClearIdleTimer { entry_id });
            }
            _ => {
                effects.push(RegistryEffect::DisposeEntry { entry_id });
            }
        }
        effects
    }

    /// TS `bindWorkspaceContext`：换绑 workspace，`generation` 加 1。
    pub fn bind_context(
        &mut self,
        remote_session_id: &str,
        workspace_path: &str,
        workspace_identity: &str,
    ) -> Result<Vec<RegistryEffect>, RegistryError> {
        let Some(session) = self.sessions.get_mut(remote_session_id) else {
            return Err(RegistryError::UnknownSession(remote_session_id.to_string()));
        };
        let entry_id = session.entry_id;
        let previous_key = session.workspace_key.clone();
        let previous_generation = session.workspace_generation;
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
        if let Some(entry) = self.entries.get_mut(&entry_id) {
            entry.workspace_keys.insert(next_key.clone());
        }
        let session_id = remote_session_id.to_string();

        // 换代际 + 登记新归属（TS 在 prepareWorkspaceRuntime 里做）。
        let mut effects = self.prepare_workspace_runtime(&session_id);

        // 换到了不同 workspace 才释放旧归属；同一 workspace 只是新的一代。
        if previous_key.as_deref() != Some(next_key.as_str()) {
            if let Some(previous) = previous_key {
                let mut release = self.request_workspace_release(
                    entry_id,
                    &session_id,
                    Some(&previous),
                    previous_generation,
                );
                effects.append(&mut release);
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
        let entry_id = session.entry_id;
        let workspace_key = session.workspace_key.clone();
        let workspace_generation = session.workspace_generation;
        if let Some(entry) = self.entries.get_mut(&entry_id) {
            entry.sessions.remove(remote_session_id);
        }

        // 先登记 workspace 释放，再决定 entry 的去留（TS 同序）。
        let mut effects = self.request_workspace_release(
            entry_id,
            remote_session_id,
            workspace_key.as_deref(),
            workspace_generation,
        );

        let Some(entry) = self.entries.get(&entry_id) else {
            return effects;
        };
        if !entry.sessions.is_empty() {
            return effects;
        }
        match (entry.target.kind(), entry.state) {
            ("wsl", ConnectionState::Online) => {
                // 只迁移 owner：ready 连接继续留在 pool 里等下一个 workspace。
                if let Some(mut idle) =
                    self.schedule_idle_dispose(entry_id, "last-logical-session-disposed")
                {
                    effects.append(&mut idle);
                }
            }
            ("ssh", ConnectionState::Online) => {
                // ready connection 继续保持 window-scoped cache；真实关窗时统一释放。
                effects.push(RegistryEffect::ClearIdleTimer { entry_id });
            }
            _ => effects.push(RegistryEffect::DisposeEntry { entry_id }),
        }
        effects
    }

    /// TS `setWorkspaceRunningTaskCount`：按 workspace 记账；归零时补做被推迟的释放。
    ///
    /// 只对 **WSL entry** 且**曾持有过该 workspace** 的 entry 生效——否则本地 workspace
    /// 的 task 计数会误伤远端 pool。
    pub fn set_workspace_running_task_count(
        &mut self,
        workspace_path: &str,
        workspace_identity: Option<&str>,
        running_task_count: u32,
    ) -> Vec<RegistryEffect> {
        let workspace_key = workspace_identity
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| workspace_path.to_string());

        let entry_ids: Vec<u64> = self
            .entries
            .iter()
            .filter(|(_, entry)| {
                entry.target.kind() == "wsl" && entry.workspace_keys.contains(&workspace_key)
            })
            .map(|(entry_id, _)| *entry_id)
            .collect();

        let mut effects = Vec::new();
        for entry_id in entry_ids {
            if running_task_count > 0 {
                if let Some(entry) = self.entries.get_mut(&entry_id) {
                    entry
                        .running_task_count
                        .insert(workspace_key.clone(), running_task_count);
                }
                effects.push(RegistryEffect::ClearIdleTimer { entry_id });
                continue;
            }

            if let Some(entry) = self.entries.get_mut(&entry_id) {
                entry.running_task_count.remove(&workspace_key);
            }
            // task 归零：补做之前因 task > 0 被推迟的那一代释放。
            let pending_generation = self
                .entries
                .get(&entry_id)
                .and_then(|entry| entry.workspace_runtime.get(&workspace_key))
                .and_then(|state| state.pending_release_generation);
            if let Some(generation) = pending_generation {
                if let Some(effect) =
                    self.start_workspace_release(entry_id, &workspace_key, generation)
                {
                    effects.push(effect);
                }
            }
        }
        effects
    }

    /// 调用方完成一次 release 后回传（TS 的 `release.finally` 分支）。
    ///
    /// 若这一代仍是当前代、没有待办释放、也没有其他 owner，就删除运行时代际
    /// ——否则同 workspace 的下一代 acquire 会以为「上一代还在飞」而永远 Pending。
    pub fn complete_workspace_release(
        &mut self,
        entry_id: u64,
        workspace_key: &str,
        generation: u32,
    ) -> Vec<RegistryEffect> {
        let Some(entry) = self.entries.get_mut(&entry_id) else {
            return Vec::new();
        };
        let Some(state) = entry.workspace_runtime.get_mut(workspace_key) else {
            return Vec::new();
        };
        state.release_in_flight = false;
        let removable = state.generation == generation
            && state.pending_release_generation.is_none()
            && !self.entry_has_workspace_owner(entry_id, workspace_key, "");
        if removable {
            if let Some(entry) = self.entries.get_mut(&entry_id) {
                entry.workspace_runtime.remove(workspace_key);
            }
        }
        Vec::new()
    }

    /// 某个 session 的 workspace 就绪态（对应 TS `session.workspaceReadyState`）。
    pub fn workspace_ready_state(&self, remote_session_id: &str) -> Option<WorkspaceReadyState> {
        self.sessions
            .get(remote_session_id)
            .map(|session| session.workspace_ready)
    }

    /// 某个 entry 上某 workspace 的当前代际（测试与诊断用）。
    pub fn workspace_generation(&self, entry_id: u64, workspace_key: &str) -> Option<u32> {
        self.entries
            .get(&entry_id)
            .and_then(|entry| entry.workspace_runtime.get(workspace_key))
            .map(|state| state.generation)
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

    /// TS `findSessionForWorkspace`：按 workspace 找唯一 session。
    ///
    /// 多个候选时**报错而不是随便挑一个**——同一 workspace 出现两个 logical session
    /// 意味着上层绑定逻辑出错，静默挑一个会把消息发到错误的会话。
    pub fn find_session_for_workspace(
        &self,
        workspace_path: &str,
        workspace_identity: Option<&str>,
    ) -> Result<Option<RemoteSessionSnapshot>, RegistryError> {
        let matches: Vec<&Session> = self
            .sessions
            .values()
            .filter(|session| {
                session.workspace_path.as_deref() == Some(workspace_path)
                    && session.workspace_identity.as_deref() == workspace_identity
            })
            .collect();
        let online: Vec<&Session> = matches
            .iter()
            .copied()
            .filter(|session| {
                session.state == ConnectionState::Online && session.source_availability == "online"
            })
            .collect();
        if online.len() > 1 || (online.is_empty() && matches.len() > 1) {
            return Err(RegistryError::AmbiguousWorkspace(
                workspace_path.to_string(),
            ));
        }
        Ok(online
            .first()
            .copied()
            .or_else(|| matches.first().copied())
            .map(Session::snapshot))
    }

    /// TS `handleConnectionClosed`：所有 session 转 disconnected/offline。
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

    /// `getStats`：`connectionCount` / `logicalSessionCount`。
    pub fn stats(&self) -> (usize, usize) {
        (self.entry_by_key.len(), self.sessions.len())
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

    // ---- workspace 运行时归属（TS prepareWorkspaceRuntime / requestWorkspaceRelease）----

    /// 登记 session 对 workspace 的归属，并推进代际。
    ///
    /// 只有 WSL 有运行时；其余 kind 直接 `Ready`（TS 同）。
    fn prepare_workspace_runtime(&mut self, remote_session_id: &str) -> Vec<RegistryEffect> {
        let Some(session) = self.sessions.get(remote_session_id) else {
            return Vec::new();
        };
        let entry_id = session.entry_id;
        let Some(workspace_key) = session.workspace_key.clone() else {
            return Vec::new();
        };
        let workspace_path = session.workspace_path.clone();
        let workspace_identity = session.workspace_identity.clone();

        let Some(entry) = self.entries.get(&entry_id) else {
            return Vec::new();
        };
        if entry.target.kind() != "wsl" {
            if let Some(session) = self.sessions.get_mut(remote_session_id) {
                session.workspace_ready = WorkspaceReadyState::Ready;
            }
            return Vec::new();
        }
        let Some(workspace_path) = workspace_path else {
            if let Some(session) = self.sessions.get_mut(remote_session_id) {
                session.workspace_ready = WorkspaceReadyState::Ready;
            }
            return Vec::new();
        };

        let has_other_owner =
            self.entry_has_workspace_owner(entry_id, &workspace_key, remote_session_id);
        let entry = self
            .entries
            .get_mut(&entry_id)
            .expect("entry 在上面的借用里已确认存在");
        let state = entry
            .workspace_runtime
            .entry(workspace_key.clone())
            .or_insert_with(|| WorkspaceRuntimeState {
                context: RemoteWorkspaceContext {
                    workspace_path: workspace_path.clone(),
                    workspace_identity: workspace_identity.clone(),
                },
                generation: 0,
                pending_release_generation: None,
                release_in_flight: false,
            });
        state.context = RemoteWorkspaceContext {
            workspace_path,
            workspace_identity,
        };
        // 没有其他 owner 才算「重新抢到」：代际 +1 让尚未开始的旧 release 失效
        // （上一代 release 可能还在等 running task 归零）。
        if !has_other_owner {
            state.generation += 1;
        }
        state.pending_release_generation = None;
        let generation = state.generation;
        let release_in_flight = state.release_in_flight;

        if let Some(session) = self.sessions.get_mut(remote_session_id) {
            session.workspace_generation = Some(generation);
            // 上一代 release 还在飞就停在 Pending，等 complete_workspace_release。
            session.workspace_ready = if release_in_flight {
                WorkspaceReadyState::Pending
            } else {
                WorkspaceReadyState::Ready
            };
        }
        Vec::new()
    }

    /// 登记一次 workspace 释放（TS `requestWorkspaceRelease`）。
    ///
    /// 四种「不释放」的情形都要原样保留，少一条就会把别人还在用的 runtime 拆掉：
    /// 非 WSL、没有 key/代际、还有别的 owner、running task > 0。
    fn request_workspace_release(
        &mut self,
        entry_id: u64,
        excluded_session_id: &str,
        workspace_key: Option<&str>,
        workspace_generation: Option<u32>,
    ) -> Vec<RegistryEffect> {
        let (Some(workspace_key), Some(generation)) = (workspace_key, workspace_generation) else {
            return Vec::new();
        };
        let Some(entry) = self.entries.get(&entry_id) else {
            return Vec::new();
        };
        if entry.target.kind() != "wsl" {
            return Vec::new();
        }
        if self.entry_has_workspace_owner(entry_id, workspace_key, excluded_session_id) {
            return Vec::new();
        }
        let Some(state) = self
            .entries
            .get(&entry_id)
            .and_then(|entry| entry.workspace_runtime.get(workspace_key))
        else {
            return Vec::new();
        };
        if state.generation != generation {
            return Vec::new();
        }
        let running = self
            .entries
            .get(&entry_id)
            .and_then(|entry| entry.running_task_count.get(workspace_key).copied())
            .unwrap_or(0);
        if running > 0 {
            // 还有 task 在跑：记下代际，等归零后补做。
            if let Some(state) = self
                .entries
                .get_mut(&entry_id)
                .and_then(|entry| entry.workspace_runtime.get_mut(workspace_key))
            {
                state.pending_release_generation = Some(generation);
            }
            return Vec::new();
        }
        self.start_workspace_release(entry_id, workspace_key, generation)
            .into_iter()
            .collect()
    }

    /// 真正发起一次释放（TS `startWorkspaceRelease`）。
    fn start_workspace_release(
        &mut self,
        entry_id: u64,
        workspace_key: &str,
        generation: u32,
    ) -> Option<RegistryEffect> {
        let entry = self.entries.get(&entry_id)?;
        if entry.target.kind() != "wsl" {
            return None;
        }
        // 连接没 online（或已换 entry）时发不出去；保持 pending 等下次触发。
        if entry.state != ConnectionState::Online {
            return None;
        }
        let state = entry.workspace_runtime.get(workspace_key)?;
        if state.generation != generation || state.release_in_flight {
            return None;
        }
        let context = state.context.clone();
        let effect = RegistryEffect::ReleaseWorkspace {
            entry_id,
            workspace_key: workspace_key.to_string(),
            generation,
            context,
        };
        if let Some(state) = self
            .entries
            .get_mut(&entry_id)
            .and_then(|entry| entry.workspace_runtime.get_mut(workspace_key))
        {
            state.pending_release_generation = None;
            state.release_in_flight = true;
        }
        Some(effect)
    }

    /// TS `scheduleWslIdleDispose`：只有 WSL、无 session、无 running task 才真排。
    fn schedule_idle_dispose(
        &mut self,
        entry_id: u64,
        reason: &str,
    ) -> Option<Vec<RegistryEffect>> {
        let entry = self.entries.get(&entry_id)?;
        if entry.target.kind() != "wsl"
            || entry.disposed
            || !entry.sessions.is_empty()
            || entry.running_task_count.values().any(|count| *count > 0)
        {
            return None;
        }
        let _ = reason;
        Some(vec![RegistryEffect::ScheduleIdleDispose { entry_id }])
    }

    /// 是否有**别的** session 仍持有该 entry 上的这个 workspace。
    fn entry_has_workspace_owner(
        &self,
        entry_id: u64,
        workspace_key: &str,
        excluded_session_id: &str,
    ) -> bool {
        self.sessions.values().any(|session| {
            session.remote_session_id != excluded_session_id
                && !session.cancelled
                && session.entry_id == entry_id
                && session.workspace_key.as_deref() == Some(workspace_key)
        })
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
                workspace_keys: BTreeSet::new(),
                workspace_runtime: HashMap::new(),
                running_task_count: HashMap::new(),
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
            server_id: None,
            token: None,
        }
    }

    fn ssh() -> RemoteTargetSpec {
        RemoteTargetSpec::Ssh {
            host: "h".to_string(),
            port: None,
            username: "u".to_string(),
            private_key_path: None,
            has_password: false,
        }
    }

    fn wsl(distro: &str) -> RemoteTargetSpec {
        RemoteTargetSpec::Wsl {
            distro: Some(distro.to_string()),
            user: None,
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

    /// 只挑出 ReleaseWorkspace 效果，断言时不必关心同批的其它 effect。
    fn releases(effects: &[RegistryEffect]) -> Vec<(String, u32)> {
        effects
            .iter()
            .filter_map(|effect| match effect {
                RegistryEffect::ReleaseWorkspace {
                    workspace_key,
                    generation,
                    ..
                } => Some((workspace_key.clone(), *generation)),
                _ => None,
            })
            .collect()
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
        // 复用 entry 时 session id 必须继续递增，不能与已有 id 撞。
        assert_ne!(first.remote_session_id, second.remote_session_id);
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
        let (plan, _) = registry
            .begin_connect(request("req-1", ssh(), Some("/w")))
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
            .begin_connect(request("req-2", ssh(), Some("/w")))
            .expect("reconnect");
        assert_ne!(next.entry_id, plan.entry_id);
    }

    #[test]
    fn cancel_keeps_online_ssh_entry_but_idles_online_wsl() {
        let mut registry = RemoteSessionRegistry::new();
        let (ssh_plan, _) = registry
            .begin_connect(request("req-1", ssh(), Some("/w")))
            .expect("ssh connect");
        registry.complete_connect(ssh_plan.entry_id);
        let ssh_effects = registry.cancel_connect("req-1");
        // online 的 SSH entry 是窗口 cache：只清 idle 定时器，**不得**销毁或 abort。
        assert!(
            ssh_effects
                .iter()
                .all(|effect| matches!(effect, RegistryEffect::ClearIdleTimer { .. })),
            "online 的 SSH entry 不该被销毁：{ssh_effects:?}"
        );
        // entry 仍在复用表里：同 key 再连必须复用（不是新建）。
        let (again, _) = registry
            .begin_connect(request("req-9", ssh(), Some("/w")))
            .expect("reuse");
        assert_eq!(again.entry_id, ssh_plan.entry_id, "ssh cache 应被复用");

        let mut registry = RemoteSessionRegistry::new();
        let (wsl_plan, _) = registry
            .begin_connect(request("req-2", wsl("Ubuntu"), Some("/w")))
            .expect("wsl connect");
        registry.complete_connect(wsl_plan.entry_id);
        let wsl_effects = registry.cancel_connect("req-2");
        assert!(
            wsl_effects
                .iter()
                .any(|effect| matches!(effect, RegistryEffect::ScheduleIdleDispose { .. })),
            "online 的 WSL 应走 idle 销毁：{wsl_effects:?}"
        );
        assert!(
            wsl_effects
                .iter()
                .all(|effect| !matches!(effect, RegistryEffect::DisposeEntry { .. })),
            "online 的 WSL 不该立刻销毁（启动贵）：{wsl_effects:?}"
        );
    }

    #[test]
    fn late_completion_never_revives_a_retired_entry() {
        let mut registry = RemoteSessionRegistry::new();
        let (plan, _) = registry
            .begin_connect(request("req-1", ssh(), Some("/w")))
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
    fn late_completion_after_failure_does_not_resurrect() {
        let mut registry = RemoteSessionRegistry::new();
        let (plan, _) = registry
            .begin_connect(request("req-1", lan("h", 1), Some("/w")))
            .expect("connect");
        registry.fail_connect(plan.entry_id);
        assert_eq!(
            registry.complete_connect(plan.entry_id),
            Vec::<RegistryEffect>::new(),
            "entry 已被 fail 移除，迟到完成什么都没得复活"
        );
        assert!(registry.list_sessions().is_empty());
    }

    #[test]
    fn bind_context_bumps_generation_but_non_wsl_has_no_workspace_runtime() {
        let mut registry = RemoteSessionRegistry::new();
        let (plan, _) = registry
            .begin_connect(request("req-1", lan("h", 1), Some("/old")))
            .expect("connect");
        registry.complete_connect(plan.entry_id);

        let effects = registry
            .bind_context(&plan.remote_session_id, "/new", "remote:lan:h:2:/new")
            .expect("bind");
        // 非 WSL 没有 workspace 运行时，所以没有 release（TS 同：requestWorkspaceRelease 早期返回）。
        assert!(releases(&effects).is_empty(), "{effects:?}");
        assert_eq!(
            registry
                .get_session(&plan.remote_session_id)
                .expect("session")
                .generation,
            2,
            "generation 是 session 的代际，每次 bind 都 +1"
        );
        // 非 WSL 的 workspace 直接就绪。
        assert_eq!(
            registry.workspace_ready_state(&plan.remote_session_id),
            Some(WorkspaceReadyState::Ready)
        );

        assert_eq!(
            registry.bind_context("missing", "/x", "y"),
            Err(RegistryError::UnknownSession("missing".to_string()))
        );
    }

    #[test]
    fn wsl_bind_context_releases_the_previous_workspace_generation() {
        let mut registry = RemoteSessionRegistry::new();
        let (plan, _) = registry
            .begin_connect(request("req-1", wsl("Ubuntu"), Some("/old")))
            .expect("connect");
        registry.complete_connect(plan.entry_id);
        assert_eq!(
            registry.workspace_generation(plan.entry_id, "/old"),
            Some(1)
        );
        assert_eq!(
            registry.workspace_ready_state(&plan.remote_session_id),
            Some(WorkspaceReadyState::Ready)
        );

        let effects = registry
            .bind_context(&plan.remote_session_id, "/new", "remote:wsl:Ubuntu:/new")
            .expect("bind");
        // 换到不同 workspace：旧归属要释放，代际是**旧**那一代（1）。
        assert_eq!(
            releases(&effects),
            vec![("/old".to_string(), 1)],
            "{effects:?}"
        );

        // 同一 workspace 再绑：只是新的一代，不释放；代际不递增（没有别的 owner 抢）。
        let effects = registry
            .bind_context(&plan.remote_session_id, "/new", "remote:wsl:Ubuntu:/new")
            .expect("rebind");
        assert!(releases(&effects).is_empty(), "{effects:?}");
    }

    #[test]
    fn wsl_release_waits_while_tasks_are_running() {
        let mut registry = RemoteSessionRegistry::new();
        let (plan, _) = registry
            .begin_connect(request("req-1", wsl("Ubuntu"), Some("/w")))
            .expect("connect");
        registry.complete_connect(plan.entry_id);

        // 有 task 在跑：release 被推迟，记住代际。
        assert!(registry
            .set_workspace_running_task_count("/w", None, 2)
            .iter()
            .all(|effect| matches!(effect, RegistryEffect::ClearIdleTimer { .. })));
        assert!(registry.dispose_session(&plan.remote_session_id).is_empty());

        // task 归零：补做被推迟的释放。
        let effects = registry.set_workspace_running_task_count("/w", None, 0);
        assert_eq!(
            releases(&effects),
            vec![("/w".to_string(), 1)],
            "{effects:?}"
        );
    }

    #[test]
    fn workspace_release_is_not_sent_twice_and_completion_clears_the_generation() {
        let mut registry = RemoteSessionRegistry::new();
        let (plan, _) = registry
            .begin_connect(request("req-2", wsl("Ubuntu"), Some("/w")))
            .expect("connect");
        registry.complete_connect(plan.entry_id);

        let effects = registry.dispose_session(&plan.remote_session_id);
        assert_eq!(releases(&effects), vec![("/w".to_string(), 1)]);

        // 还在飞的时候再请求释放：不得重复发起（否则同一 runtime 被拆两次）。
        registry
            .begin_connect(request("req-3", wsl("Ubuntu"), Some("/w")))
            .expect("second connect");
        assert!(registry
            .complete_workspace_release(plan.entry_id, "/w", 1)
            .is_empty());
    }

    #[test]
    fn pending_release_state_keeps_next_owner_pending_until_release_completes() {
        let mut registry = RemoteSessionRegistry::new();
        let (first, _) = registry
            .begin_connect(request("req-1", wsl("Ubuntu"), Some("/w")))
            .expect("connect");
        registry.complete_connect(first.entry_id);
        let task_effects = registry.set_workspace_running_task_count("/w", None, 1);
        assert!(
            task_effects
                .iter()
                .all(|effect| matches!(effect, RegistryEffect::ClearIdleTimer { .. })),
            "有 task 在跑时应清 idle 定时器：{task_effects:?}"
        );
        let effects = registry.dispose_session(&first.remote_session_id);
        assert!(releases(&effects).is_empty(), "task 未归零不该发起释放");

        // 新 owner 抢同一个 workspace：代际 +1，让尚未开始的旧 release 失效。
        let (second, _) = registry
            .begin_connect(request("req-2", wsl("Ubuntu"), Some("/w")))
            .expect("reuse entry");
        assert_eq!(
            registry.workspace_generation(second.entry_id, "/w"),
            Some(2),
            "新 owner 抢到后必须换代际"
        );

        // task 归零：旧代际 (1) 的待办释放要失效，不能把新 owner 的 runtime 拆掉。
        let effects = registry.set_workspace_running_task_count("/w", None, 0);
        assert!(
            releases(&effects).is_empty(),
            "旧代际的待办释放必须失效：{effects:?}"
        );
        assert_eq!(
            registry.workspace_ready_state(&second.remote_session_id),
            Some(WorkspaceReadyState::Ready)
        );
    }

    #[test]
    fn identity_wins_over_path_in_the_workspace_key() {
        let mut registry = RemoteSessionRegistry::new();
        let (plan, _) = registry
            .begin_connect(ConnectRequest {
                request_id: "req-1".to_string(),
                target: wsl("Ubuntu"),
                workspace_path: Some("/w".to_string()),
                workspace_identity: Some("  remote:wsl:Ubuntu:/w  ".to_string()),
            })
            .expect("connect");
        registry.complete_connect(plan.entry_id);
        let effects = registry
            .bind_context(&plan.remote_session_id, "/w2", " ")
            .expect("bind");
        // identity 为空时退回 path；旧 key 是 trim 过的 identity。
        assert_eq!(
            releases(&effects),
            vec![("remote:wsl:Ubuntu:/w".to_string(), 1)],
            "{effects:?}"
        );
    }

    #[test]
    fn failure_removes_entry_and_sessions() {
        let mut registry = RemoteSessionRegistry::new();
        let (plan, _) = registry
            .begin_connect(request("req-1", lan("h", 1), Some("/w")))
            .expect("connect");
        let effects = registry.fail_connect(plan.entry_id);
        assert!(
            effects.is_empty(),
            "失败不需要调用方做额外清理：{effects:?}"
        );
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
    fn find_session_for_workspace_rejects_ambiguity() {
        let mut registry = RemoteSessionRegistry::new();
        // docker 不复用 entry，所以同一 workspace 能出现两个 session。
        let docker = RemoteTargetSpec::Docker {
            container: "box".to_string(),
        };
        let (first, _) = registry
            .begin_connect(request("req-1", docker.clone(), Some("/w")))
            .expect("first");
        registry.complete_connect(first.entry_id);
        assert_eq!(
            registry
                .find_session_for_workspace("/w", None)
                .expect("unique")
                .map(|snapshot| snapshot.remote_session_id),
            Some(first.remote_session_id.clone())
        );

        let (second, _) = registry
            .begin_connect(request("req-2", docker, Some("/w")))
            .expect("second");
        registry.complete_connect(second.entry_id);
        assert_eq!(
            registry.find_session_for_workspace("/w", None),
            Err(RegistryError::AmbiguousWorkspace("/w".to_string())),
            "两个候选必须报错，不能静默挑一个把消息发错会话"
        );
        assert!(registry
            .find_session_for_workspace("/nope", None)
            .expect("none")
            .is_none());
    }

    #[test]
    fn stats_counts_connections_and_sessions_separately() {
        let mut registry = RemoteSessionRegistry::new();
        assert_eq!(registry.stats(), (0, 0));
        registry
            .begin_connect(request("req-1", lan("h", 1), Some("/a")))
            .expect("first");
        registry
            .begin_connect(request("req-2", lan("h", 1), Some("/b")))
            .expect("second");
        // 同 target 复用：1 条连接、2 个 logical session。
        assert_eq!(registry.stats(), (1, 2));
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
