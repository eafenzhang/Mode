//! 壳内频道接线（M4b W0-4b）。
//!
//! 设计见 `docs/plans/2026-10-29-m4b-host-into-shell.md` §「壳内接线设计」：
//!
//! - `host_send` 先问 [`InprocessState::decide`]：本地频道 → 投给消费任务；畸形帧 → 丢弃；
//!   其余 → 原样转发给 Node Host（逐字节不变）。
//! - 消费任务**独占** `ChannelServer`，串行处理入站帧（配 `handle_frame` 内部的并发执行），
//!   因此不需要让同步的 Tauri 命令进入 tokio 运行时。
//! - 本地回帧经 `HostHandle::push_local_frame` 注入同一入方向队列，`host_recv` 不用改。
//! - 服务面按 **Host 代**重建：换代时 `dispose` 旧服务器，避免本地与对端 id 空间错位。
//! - `ChannelServer` 用 `defer_init = true` 且永不 `ready()`：`Initialize` 只能由 Node Host 发。
//!
//! 开关：`MODE_HOST_INPROCESS_CHANNELS`。未设置/`none` = 纯转发；`default` = 全部白名单
//! 业务频道（不含 `mode-agent`，S2 另有门禁）。语义与默认翻转前置见
//! `docs/specs/w2-transport-cutover.md` §3/§5。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use agent_process::AgentControlClient;
use host_channels::{
    AGENT_HEALTH_CHANNEL, ATTACHMENT_TRANSFER_CHANNEL, AttachmentTransferChannel,
    BROADCAST_CHANNEL, BroadcastChannel, CLIENT_CONFIG_CHANNEL, CLIENT_SCENES_CHANNEL,
    COMMANDS_CHANNEL, CONVERSATION_MIRROR_CHANNEL, CREDENTIAL_CHANNEL, ClientConfigChannel,
    ClientScenesChannel, CommandsChannel, ConversationMirrorChannel, CredentialChannel,
    FILE_CHANNEL, FILE_WATCHER_CHANNEL, FileChannel, FileWatcherChannel, GIT_CHANNEL,
    GIT_CHECKPOINT_CHANNEL, GitChannel, GitCheckpointChannel, MEDIA_PREVIEW_CHANNEL,
    MEMORY_CHANNEL, MediaPreviewChannel, MemoryChannel, PLUGIN_SYNC_CHANNEL, PluginSyncChannel,
    SETTING_CHANNEL, SETTINGS_SYNC_CHANNEL, SKILL_SYNC_CHANNEL, SKILLS_CHANNEL, SUBAGENTS_CHANNEL,
    SYSTEM_CHANNEL, SettingChannel, SettingsSyncChannel, SkillSyncChannel, SkillsChannel,
    SubagentsChannel, SystemChannel, TERMINAL_CHANNEL, TerminalChannel,
};
use host_channels::{
    AgentHealthChannel, HOOKS_CHANNEL, HooksChannel, MCP_SYNC_CHANNEL, MODE_AGENT_CHANNEL,
    McpSyncChannel, ModeAgentChannel, USAGE_STATS_CHANNEL, UsageStatsChannel,
};
use project_memory::ProjectMemoryService;
use rpc_host::{ChannelServer, FrameRoute, FrameRouter, LocalScopeRegistry};
use std::sync::Arc as StdArc;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::host::HostSupervisor;

const ENV_CHANNELS: &str = "MODE_HOST_INPROCESS_CHANNELS";

/// 一帧的去向。
pub enum RouteDecision {
    /// 本地服务面处理；带上剥掉 scope 前缀后的原始 payload 与所属 scope。
    Local {
        scope_id: Option<u32>,
        payload: Vec<u8>,
    },
    /// 已判定为不可处理（畸形帧/非请求）：丢弃，不回帧也不转发。
    Dropped,
    /// 交给遗留 Node Host（逐字节转发）。
    Forward,
}

pub struct InprocessState {
    router: FrameRouter,
    local_tx: UnboundedSender<(Option<u32>, Vec<u8>)>,
    enabled: bool,
    /// S2 翻面/回滚与退出清理共用同一个 client；健康面只读，不会因持有它而 spawn。
    agent_control: Arc<AgentControlClient>,
    /// 页面桥声明挂接/回收的 remote scope。scope 生命周期归 Node Host：登记表是
    /// decide 的**门禁**——只有已挂接 scope 上的请求才由壳内服务面应答，未挂接/
    /// 已 detach（含登记表不可读时 fail-closed）一律回落 Node（见 decide_route）。
    scopes: std::sync::Mutex<LocalScopeRegistry>,
}

/// 开关允许接管的频道集合。`mode-agent` / `agent-health` 与普通业务频道一样按名字
/// 显式加入；漏在这里的表现是「日志说 ignored、频道永远不注册」。
fn is_owned_channel(name: &str) -> bool {
    matches!(
        name,
        MEMORY_CHANNEL
            | ATTACHMENT_TRANSFER_CHANNEL
            | CLIENT_CONFIG_CHANNEL
            | CLIENT_SCENES_CHANNEL
            | MEDIA_PREVIEW_CHANNEL
            | CONVERSATION_MIRROR_CHANNEL
            | SETTING_CHANNEL
            | CREDENTIAL_CHANNEL
            | SYSTEM_CHANNEL
            | FILE_CHANNEL
            | FILE_WATCHER_CHANNEL
            | COMMANDS_CHANNEL
            | SKILLS_CHANNEL
            | SUBAGENTS_CHANNEL
            | SETTINGS_SYNC_CHANNEL
            | BROADCAST_CHANNEL
            | SKILL_SYNC_CHANNEL
            | GIT_CHANNEL
            | GIT_CHECKPOINT_CHANNEL
            | PLUGIN_SYNC_CHANNEL
            | USAGE_STATS_CHANNEL
            | MCP_SYNC_CHANNEL
            | AGENT_HEALTH_CHANNEL
            | MODE_AGENT_CHANNEL
            | HOOKS_CHANNEL
            | TERMINAL_CHANNEL
    )
}

/// `default` 特值展开的频道集：全部白名单业务频道，**不含 `mode-agent`**——
/// 它的频道所有权翻面挂 W4 S2 的 20 任务验收（spec docs/specs/host-ownership-switch.md），
/// 不随 W2 默认集翻面。`host` 永不在内（看门狗不能测壳自己）。
/// 规范：docs/specs/w2-transport-cutover.md §3。
fn default_owned_channels() -> Vec<String> {
    [
        MEMORY_CHANNEL,
        ATTACHMENT_TRANSFER_CHANNEL,
        CLIENT_CONFIG_CHANNEL,
        CLIENT_SCENES_CHANNEL,
        MEDIA_PREVIEW_CHANNEL,
        CONVERSATION_MIRROR_CHANNEL,
        SETTING_CHANNEL,
        CREDENTIAL_CHANNEL,
        SYSTEM_CHANNEL,
        FILE_CHANNEL,
        FILE_WATCHER_CHANNEL,
        COMMANDS_CHANNEL,
        SKILLS_CHANNEL,
        SUBAGENTS_CHANNEL,
        SETTINGS_SYNC_CHANNEL,
        BROADCAST_CHANNEL,
        SKILL_SYNC_CHANNEL,
        GIT_CHANNEL,
        GIT_CHECKPOINT_CHANNEL,
        PLUGIN_SYNC_CHANNEL,
        USAGE_STATS_CHANNEL,
        MCP_SYNC_CHANNEL,
        AGENT_HEALTH_CHANNEL,
        HOOKS_CHANNEL,
        TERMINAL_CHANNEL,
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// 开关值的解析（纯函数，便于对拍）：
/// - `None`（未设置）→ 空集 = 纯转发（默认翻转挂真机验收，见 spec w2-transport-cutover.md §5）；
/// - `none` → 空集（显式回滚，优先级最高，出现即忽略其余项）；
/// - 含 `default` → 默认集 ∪ 显式列出的频道名（去重保序）；
/// - 其余 → 原样列表（白名单过滤与 ignored 留痕仍由 `from_env` 负责）。
fn resolve_channel_requests(raw: Option<&str>) -> Vec<String> {
    let requested: Vec<String> = raw
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect();
    if requested.iter().any(|name| name == "none") {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    for name in requested {
        if name == "default" {
            for channel in default_owned_channels() {
                if !out.iter().any(|existing| existing == &channel) {
                    out.push(channel);
                }
            }
            continue;
        }
        if !out.iter().any(|existing| existing == &name) {
            out.push(name);
        }
    }
    out
}

/// 路由裁决的纯函数核（`decide` 的可测部分；Control 的登记副作用由调用方处理）。
///
/// owned 频道的 scoped 帧只有在 scope **已挂接**时才走本地；未挂接 / 已 detach /
/// 登记表不可读（fail-closed）一律回落 Node——scope 生命周期归 Node Host，
/// 幽灵 scope 不能被本地幽灵服务应答（修复：此前登记表只写不读，detach 后的
/// scoped 请求仍会被本地应答）。
fn decide_route(route: FrameRoute, attached: Option<&LocalScopeRegistry>) -> RouteDecision {
    match route {
        FrameRoute::Local {
            payload, scope_id, ..
        } => {
            if let Some(scope) = scope_id {
                let is_attached = match attached {
                    Some(registry) => registry.is_attached(scope),
                    None => false,
                };
                if !is_attached {
                    return RouteDecision::Forward;
                }
            }
            RouteDecision::Local { scope_id, payload }
        }
        FrameRoute::Rejected => RouteDecision::Dropped,
        FrameRoute::Legacy | FrameRoute::Control(_) => RouteDecision::Forward,
    }
}

impl InprocessState {
    /// 读开关并起消费/回帧两条任务。开关为空时不注册任何频道，链路退化为纯转发。
    pub fn from_env(
        supervisor: Arc<HostSupervisor>,
        data_root: PathBuf,
        agent_control: Arc<AgentControlClient>,
    ) -> Arc<Self> {
        let mut owned = Vec::new();
        for name in resolve_channel_requests(std::env::var(ENV_CHANNELS).ok().as_deref()) {
            if is_owned_channel(&name) {
                owned.push(name);
            } else {
                // 还没移植的频道明确忽略：宁可不接管，也不能出现「半个频道」。
                crate::host::shell_log(&format!(
                    "inprocess channel ignored (not ported yet): {name}"
                ));
            }
        }
        let enabled = !owned.is_empty();
        let router = FrameRouter::new(owned.iter().cloned());
        let (local_tx, local_rx) = unbounded_channel::<(Option<u32>, Vec<u8>)>();

        if enabled {
            let (out_tx, mut out_rx) = unbounded_channel::<Vec<u8>>();
            tauri::async_runtime::spawn(run_consumer(
                Arc::clone(&supervisor),
                owned.clone(),
                data_root,
                Arc::clone(&agent_control),
                local_rx,
                out_tx,
            ));
            let forward_supervisor = Arc::clone(&supervisor);
            tauri::async_runtime::spawn(async move {
                while let Some(frame) = out_rx.recv().await {
                    let supervisor = Arc::clone(&forward_supervisor);
                    let pushed = tauri::async_runtime::spawn_blocking(move || {
                        supervisor.current().push_local_frame(frame)
                    })
                    .await;
                    match pushed {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => {
                            crate::host::shell_log(&format!("inprocess frame dropped: {error}"))
                        }
                        Err(error) => crate::host::shell_log(&format!(
                            "inprocess forward task failed: {error}"
                        )),
                    }
                }
            });
            // release 是 windows_subsystem="windows"：eprintln 没有 stderr 落地，
            // 这条诊断必须走壳侧结构化日志，否则「开没开、丢了帧」都不可观察。
            crate::host::shell_log(&format!("inprocess channels enabled: {}", owned.join(",")));
        }

        Arc::new(Self {
            router,
            local_tx,
            enabled,
            agent_control,
            scopes: std::sync::Mutex::new(LocalScopeRegistry::new()),
        })
    }

    /// 壳退出时收掉 Rust 侧 Agent 运行时（带优雅关闭宽限，超时整树强杀）。
    /// 未启用任何壳内频道时也是安全的：registry 为空，直接返回。
    pub fn shutdown_agents(&self, grace: Duration) {
        tauri::async_runtime::block_on(self.agent_control.dispose_all(grace));
    }

    pub fn decide(&self, frame: &[u8]) -> RouteDecision {
        if !self.enabled {
            return RouteDecision::Forward;
        }
        let route = self.router.route(frame);
        if let FrameRoute::Control(control) = &route {
            // 控制帧仍要转发给 Node（scope 生命周期归它），这里同步壳内登记表。
            if let Ok(mut scopes) = self.scopes.lock() {
                match control {
                    rpc_wire::AttachmentControl::ScopeReady(scope_id) => {
                        scopes.attach(*scope_id);
                    }
                    rpc_wire::AttachmentControl::ScopeDetach(scope_id) => {
                        scopes.detach(*scope_id);
                    }
                }
            }
            return RouteDecision::Forward;
        }
        let guard = self.scopes.lock().ok();
        decide_route(
            route,
            guard
                .as_deref()
                .map(|registry| registry as &LocalScopeRegistry),
        )
    }

    pub fn dispatch(&self, scope_id: Option<u32>, payload: Vec<u8>) {
        if self.local_tx.send((scope_id, payload)).is_err() {
            crate::host::shell_log("inprocess consumer is gone; frame dropped");
        }
    }
}

async fn run_consumer(
    supervisor: Arc<HostSupervisor>,
    owned: Vec<String>,
    data_root: PathBuf,
    agent_control: StdArc<AgentControlClient>,
    mut rx: UnboundedReceiver<(Option<u32>, Vec<u8>)>,
    out_tx: UnboundedSender<Vec<u8>>,
) {
    let mut generation = supervisor.current().startup_id().to_string();
    let mut base = build_server(out_tx.clone(), &owned, &data_root, &agent_control);
    let mut scoped: std::collections::HashMap<u32, ChannelServer> =
        std::collections::HashMap::new();
    while let Some((scope_id, payload)) = rx.recv().await {
        let current = supervisor.current().startup_id().to_string();
        if current != generation {
            // 换代：对端 id 空间从 0 重新计数，base 与各 scope 的 active 请求都必须释放。
            base.dispose();
            for (_, mut server) in scoped.drain() {
                server.dispose();
            }
            base = build_server(out_tx.clone(), &owned, &data_root, &agent_control);
            generation = current;
        }
        match scope_id {
            None => base.handle_frame(&payload),
            Some(scope) => {
                let server = scoped.entry(scope).or_insert_with(|| {
                    build_server(
                        scoped_out(&out_tx, scope),
                        &owned,
                        &data_root,
                        &agent_control,
                    )
                });
                server.handle_frame(&payload);
            }
        }
    }
    base.dispose();
    for (_, mut server) in scoped.drain() {
        server.dispose();
    }
}

/// 每个 remote scope 一个出站包装：回帧必须带 `0xFF + scopeId` 前缀，
/// 否则对端会当成 base 帧错投。
fn scoped_out(out: &UnboundedSender<Vec<u8>>, scope_id: u32) -> UnboundedSender<Vec<u8>> {
    let (tx, mut rx) = unbounded_channel::<Vec<u8>>();
    let out = out.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(payload) = rx.recv().await {
            match rpc_wire::scope::encode_scoped_frame(scope_id, &payload) {
                Ok(frame) => {
                    let _ = out.send(frame);
                }
                Err(error) => crate::host::shell_log(&format!(
                    "inprocess scope wrap failed scope={scope_id}: {error:?}"
                )),
            }
        }
    });
    tx
}

fn build_server(
    out_tx: UnboundedSender<Vec<u8>>,
    owned: &[String],
    data_root: &Path,
    agent_control: &StdArc<AgentControlClient>,
) -> ChannelServer {
    let mut server = ChannelServer::new(
        out_tx,
        "mode-host-sidecar",
        Duration::from_millis(1000),
        true,
    );
    for name in owned {
        match name.as_str() {
            MEMORY_CHANNEL => server.register_channel(
                MEMORY_CHANNEL,
                Arc::new(MemoryChannel::new(Arc::new(ProjectMemoryService::new(
                    data_root.to_path_buf(),
                )))),
            ),
            BROADCAST_CHANNEL => server.register_channel(
                BROADCAST_CHANNEL,
                Arc::new(BroadcastChannel::new(Arc::new(
                    broadcast_service::BroadcastService::new(),
                ))),
            ),
            ATTACHMENT_TRANSFER_CHANNEL => server.register_channel(
                ATTACHMENT_TRANSFER_CHANNEL,
                Arc::new(AttachmentTransferChannel),
            ),
            CLIENT_CONFIG_CHANNEL => {
                server.register_channel(CLIENT_CONFIG_CHANNEL, Arc::new(ClientConfigChannel))
            }
            CLIENT_SCENES_CHANNEL => {
                server.register_channel(CLIENT_SCENES_CHANNEL, Arc::new(ClientScenesChannel))
            }
            MEDIA_PREVIEW_CHANNEL => {
                server.register_channel(MEDIA_PREVIEW_CHANNEL, Arc::new(MediaPreviewChannel))
            }
            CONVERSATION_MIRROR_CHANNEL => server.register_channel(
                CONVERSATION_MIRROR_CHANNEL,
                Arc::new(ConversationMirrorChannel::new(
                    conversation_mirror::ConversationMirrorService::new(data_root.to_path_buf()),
                )),
            ),
            SETTING_CHANNEL => {
                let env = settings_store::HomeEnv::from_process();
                let paths = settings_store::resolve_settings_paths(&env);
                let home = paths.home.clone();
                let current_base = data_root
                    .parent()
                    .map(std::path::Path::to_path_buf)
                    .unwrap_or_else(|| home.clone());
                server.register_channel(
                    SETTING_CHANNEL,
                    Arc::new(SettingChannel::new(
                        Arc::new(settings_store::SettingsService::new(paths, current_base)),
                        home,
                    )),
                );
            }
            CREDENTIAL_CHANNEL => {
                // 坏凭据文件必须留下可观察的告警：服务层日志走不了 Node 的 logger，
                // 用壳侧结构化日志记录路径与备份名（不记录任何值）。
                let store = credential_store::CredentialStore::new(data_root)
                    .with_corrupt_listener(Arc::new(|path, backup| {
                        crate::host::shell_log(&format!(
                            "credential store corrupt: path={} backup={}",
                            path.display(),
                            backup
                                .map(|path| path.display().to_string())
                                .unwrap_or_else(|| "<none>".to_string())
                        ));
                    }));
                server.register_channel(
                    CREDENTIAL_CHANNEL,
                    Arc::new(CredentialChannel::new(Arc::new(store))),
                );
            }
            SYSTEM_CHANNEL => server.register_channel(
                SYSTEM_CHANNEL,
                Arc::new(SystemChannel::new(Arc::new(
                    system_service::SystemService::new(),
                ))),
            ),
            FILE_CHANNEL => {
                // 索引降级（规则文件不可用/解析失败）必须可观察；日志只带原因，不带文件内容。
                let mut options =
                    file_service::FileServiceOptions::from_process(data_root.to_path_buf());
                options.on_warn = Some(Arc::new(|message: &str| {
                    crate::host::shell_log(message);
                }));
                server.register_channel(
                    FILE_CHANNEL,
                    Arc::new(FileChannel::new(Arc::new(file_service::FileService::new(
                        options,
                    )))),
                );
            }
            FILE_WATCHER_CHANNEL => {
                let service = file_watcher::FileWatcherService::with_warn_sink(Some(Arc::new(
                    |message: &str| crate::host::shell_log(message),
                )));
                server.register_channel(
                    FILE_WATCHER_CHANNEL,
                    Arc::new(FileWatcherChannel::new(Arc::new(service))),
                );
            }
            COMMANDS_CHANNEL => {
                // 命令域的真源以**用户 home** 为根（`{home}/.mode/commands`、
                // `{home}/.mode/cli/config.json`），自定义数据目录（`data_root`）不参与，
                // 与 TS `resolveUserHomeDir()` 一致；因此这里不传 `data_root`。
                server.register_channel(
                    COMMANDS_CHANNEL,
                    Arc::new(CommandsChannel::new(Arc::new(
                        commands_service::CommandsService::new(
                            commands_service::CommandsServiceOptions::from_process(),
                        ),
                    ))),
                );
            }
            SKILLS_CHANNEL => {
                // 用户级技能在桌面运行时始终可用：Node Host 显式传 `{ isDesktopRuntime: true }`，
                // 壳内没有 `MODE_PROCESS_LABEL`，不能依赖 `from_process()` 的默认判定。
                let mut options = skills_service::SkillsServiceOptions::from_process();
                options.is_desktop_runtime = true;
                server.register_channel(
                    SKILLS_CHANNEL,
                    Arc::new(SkillsChannel::new(Arc::new(
                        skills_service::SkillsService::new(options),
                    ))),
                );
            }
            SUBAGENTS_CHANNEL => {
                let mut options = subagents_service::SubagentsServiceOptions::from_process();
                options.is_desktop_runtime = Some(true);
                server.register_channel(
                    SUBAGENTS_CHANNEL,
                    Arc::new(SubagentsChannel::new(Arc::new(
                        subagents_service::SubagentsService::new(options),
                    ))),
                );
            }
            SETTINGS_SYNC_CHANNEL => {
                // 首启提示状态落在 `setting.json`：与 `setting` 频道用同一套路径解析
                // （坏文件重试与原子写都在 `settings_store` 内），只是各持一个无状态实例。
                let env = settings_store::HomeEnv::from_process();
                let paths = settings_store::resolve_settings_paths(&env);
                let current_base = data_root
                    .parent()
                    .map(std::path::Path::to_path_buf)
                    .unwrap_or_else(|| paths.home.clone());
                let settings = Arc::new(settings_store::SettingsService::new(paths, current_base));
                server.register_channel(
                    SETTINGS_SYNC_CHANNEL,
                    Arc::new(SettingsSyncChannel::new(Arc::new(
                        settings_sync_service::SettingsSyncService::new(
                            settings_sync_service::SettingsSyncServiceOptions::default(),
                            settings,
                        ),
                    ))),
                );
            }
            SKILL_SYNC_CHANNEL => server.register_channel(
                SKILL_SYNC_CHANNEL,
                Arc::new(SkillSyncChannel::new(Arc::new(
                    skill_sync_service::SkillSyncService::new(
                        skill_sync_service::SkillSyncServiceOptions::default(),
                    ),
                ))),
            ),
            GIT_CHANNEL => server.register_channel(
                GIT_CHANNEL,
                Arc::new(GitChannel::new(host_channels::git::default_service())),
            ),
            GIT_CHECKPOINT_CHANNEL => server.register_channel(
                GIT_CHECKPOINT_CHANNEL,
                Arc::new(GitCheckpointChannel::new(
                    host_channels::git_checkpoint::default_service(data_root.to_path_buf()),
                )),
            ),
            PLUGIN_SYNC_CHANNEL => server.register_channel(
                PLUGIN_SYNC_CHANNEL,
                Arc::new(PluginSyncChannel::new(
                    host_channels::plugin_sync::default_service(),
                )),
            ),
            USAGE_STATS_CHANNEL => {
                // 余额查询凭据源：Personal 层仓库（1s 轮询 LKG）。路径显式 env 优先，
                // 回退 data_root/v2（与 agent_runtime 的 ProviderPaths fallback 同口径）。
                // 仓库不可用时不阻塞频道注册：退化为 default（一律 unsupported）。
                let personal_path = std::env::var(
                    provider_config_repository::MODE_PERSONAL_PROVIDER_CONFIG_FILE_ENV,
                )
                .ok()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    data_root
                        .join("v2")
                        .join(provider_config_repository::PERSONAL_PROVIDER_CONFIG_FILE_NAME)
                });
                match provider_config_repository::PersonalConfigRepository::new(
                    provider_config_repository::PersonalConfigRepositoryOptions {
                        file_path: personal_path,
                        import_legacy: None,
                        on_recovery: None,
                        on_polling_error: None,
                        polling_interval: provider_config_repository::PollingInterval::Millis(1_000),
                    },
                ) {
                    Ok(repository) => {
                        // 首次读取同时启动轮询线程（与 AccountFactsRuntime 同款）。
                        let repository = StdArc::new(repository);
                        let _ = repository.read();
                        match UsageStatsChannel::with_personal_layer(StdArc::clone(&repository)) {
                            Ok(channel) => {
                                let _repository = repository;
                                server.register_channel(
                                    USAGE_STATS_CHANNEL,
                                    Arc::new(channel),
                                );
                            }
                            Err(error) => {
                                crate::host::shell_log(&format!(
                                    "usage-stats channel degraded to unsupported: {error}"
                                ));
                                let _repository = repository;
                                server.register_channel(
                                    USAGE_STATS_CHANNEL,
                                    Arc::new(UsageStatsChannel::default_channel()),
                                );
                            }
                        }
                    }
                    Err(error) => {
                        crate::host::shell_log(&format!(
                            "usage-stats personal config unavailable: {error}; channel degraded to unsupported"
                        ));
                        server.register_channel(
                            USAGE_STATS_CHANNEL,
                            Arc::new(UsageStatsChannel::default_channel()),
                        );
                    }
                }
            }
            MCP_SYNC_CHANNEL => {
                server.register_channel(
                    MCP_SYNC_CHANNEL,
                    Arc::new(McpSyncChannel::from_process(Arc::clone(agent_control))),
                );
            }
            MODE_AGENT_CHANNEL => {
                // S2（host-ownership-switch §5）：默认不注册，只有开关显式包含
                // mode-agent 时才接管；未迁移方法返回 unsupported_method。
                server.register_channel(
                    MODE_AGENT_CHANNEL,
                    Arc::new(ModeAgentChannel::new(
                        Arc::clone(agent_control),
                        data_root.join(".mode").join("plugin-workspace"),
                    )),
                );
            }
            AGENT_HEALTH_CHANNEL => {
                server.register_channel(
                    AGENT_HEALTH_CHANNEL,
                    Arc::new(AgentHealthChannel::new(Arc::clone(agent_control))),
                );
            }
            HOOKS_CHANNEL => {
                server.register_channel(
                    HOOKS_CHANNEL,
                    Arc::new(HooksChannel::new(Arc::clone(agent_control))),
                );
            }
            TERMINAL_CHANNEL => {
                let env = settings_store::HomeEnv::from_process();
                let paths = settings_store::resolve_settings_paths(&env);
                let current_base = data_root
                    .parent()
                    .map(std::path::Path::to_path_buf)
                    .unwrap_or_else(|| paths.home.clone());
                let settings = Arc::new(settings_store::SettingsService::new(paths, current_base));
                server.register_channel(
                    TERMINAL_CHANNEL,
                    Arc::new(TerminalChannel::new(
                        host_channels::terminal::default_service(settings),
                    )),
                );
            }
            other => {
                crate::host::shell_log(&format!("inprocess channel not constructible: {other}"))
            }
        }
    }
    server
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_and_mcp_channels_are_explicitly_switchable() {
        // S0/S2 的频道必须能通过同一个开关启用；漏掉时只会打 ignored 日志。
        assert!(is_owned_channel(MCP_SYNC_CHANNEL));
        assert!(is_owned_channel(AGENT_HEALTH_CHANNEL));
        assert!(is_owned_channel(MODE_AGENT_CHANNEL));
        assert!(is_owned_channel(HOOKS_CHANNEL));
        assert!(is_owned_channel(TERMINAL_CHANNEL));
    }

    #[test]
    fn host_owned_channels_stay_out_of_the_switch() {
        assert!(!is_owned_channel("host"));
        assert!(!is_owned_channel("mode-task"));
        assert!(!is_owned_channel("window-controller"));
    }

    #[test]
    fn unset_switch_stays_forward_only() {
        // W2 默认翻转的前置是真机验收（spec w2-transport-cutover.md §5）；
        // 在那之前「未设置」必须保持纯转发，回归面为零。
        assert!(resolve_channel_requests(None).is_empty());
    }

    #[test]
    fn none_wins_and_disables_everything() {
        // 显式回滚语义最强：与其它项混用时以 none 为准。
        assert!(resolve_channel_requests(Some("none")).is_empty());
        assert!(resolve_channel_requests(Some("memory, none ,git")).is_empty());
    }

    #[test]
    fn default_expands_without_mode_agent_or_host() {
        let resolved = resolve_channel_requests(Some("default"));
        assert!(!resolved.is_empty());
        assert!(!resolved.iter().any(|name| name == MODE_AGENT_CHANNEL));
        assert!(!resolved.iter().any(|name| name == "host"));
        for channel in [
            MEMORY_CHANNEL,
            TERMINAL_CHANNEL,
            MCP_SYNC_CHANNEL,
            HOOKS_CHANNEL,
            AGENT_HEALTH_CHANNEL,
        ] {
            assert!(
                resolved.iter().any(|name| name == channel),
                "{channel} 缺席"
            );
        }
    }

    #[test]
    fn default_merges_with_explicit_names_and_dedupes() {
        let resolved = resolve_channel_requests(Some("default, memory, host, mode-task"));
        // default 已含 memory；host/mode-task 不在白名单，由 from_env 的过滤留痕，
        // 但解析层如实保留显式名字（职责分离：解析不认识白名单）。
        assert!(resolved.iter().any(|name| name == MEMORY_CHANNEL));
        assert_eq!(
            resolved
                .iter()
                .filter(|name| name.as_str() == MEMORY_CHANNEL)
                .count(),
            1,
            "memory 不应重复"
        );
        assert!(resolved.iter().any(|name| name == "host"));
        assert!(resolved.iter().any(|name| name == "mode-task"));
    }

    fn memory_list_frame(id: i32) -> Vec<u8> {
        use rpc_wire::{RequestHeader, RpcMessage, Value, encode_rpc_message};
        encode_rpc_message(&RpcMessage {
            header: RequestHeader::Promise {
                id,
                channel: MEMORY_CHANNEL.into(),
                method: "listProjectMemories".into(),
            },
            body: Value::Array(Vec::new()),
        })
    }

    #[test]
    fn scoped_requests_forward_until_the_scope_is_attached() {
        // scope 生命周期归 Node（spec w2-transport-cutover.md §4）：scope-ready 之前/
        // detach 之后，scoped 请求回落 Node；挂接期间由本地应答。
        let router = FrameRouter::new([MEMORY_CHANNEL]);
        let mut registry = LocalScopeRegistry::new();
        let scoped =
            rpc_wire::encode_scoped_frame(9, &memory_list_frame(1)).expect("编码 scoped 帧");

        assert!(matches!(
            decide_route(router.route(&scoped), Some(&registry)),
            RouteDecision::Forward
        ));
        registry.attach(9);
        assert!(matches!(
            decide_route(router.route(&scoped), Some(&registry)),
            RouteDecision::Local {
                scope_id: Some(9),
                ..
            }
        ));
        registry.detach(9);
        assert!(matches!(
            decide_route(router.route(&scoped), Some(&registry)),
            RouteDecision::Forward
        ));
    }

    #[test]
    fn base_requests_stay_local_and_garbage_drops() {
        let router = FrameRouter::new([MEMORY_CHANNEL]);
        assert!(matches!(
            decide_route(router.route(&memory_list_frame(2)), None),
            RouteDecision::Local { scope_id: None, .. }
        ));
        assert!(matches!(
            decide_route(router.route(&[1, 2, 3]), None),
            RouteDecision::Dropped
        ));
    }

    #[test]
    fn unreadable_registry_fails_closed_to_node() {
        // 登记表锁损坏时宁可回落 Node 也不能本地应答幽灵 scope。
        let router = FrameRouter::new([MEMORY_CHANNEL]);
        let scoped =
            rpc_wire::encode_scoped_frame(7, &memory_list_frame(3)).expect("编码 scoped 帧");
        assert!(matches!(
            decide_route(router.route(&scoped), None),
            RouteDecision::Forward
        ));
    }
}
