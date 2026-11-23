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
    build_channel, AGENT_HEALTH_CHANNEL, ATTACHMENT_TRANSFER_CHANNEL, BROADCAST_CHANNEL,
    CLIENT_CONFIG_CHANNEL, CLIENT_SCENES_CHANNEL, COMMANDS_CHANNEL, CONVERSATION_MIRROR_CHANNEL,
    CREDENTIAL_CHANNEL, FILE_CHANNEL, FILE_WATCHER_CHANNEL, GIT_CHANNEL,
    MODEL_SELECTION_CHANNEL,
    MODE_SESSION_CHANNEL,
    ONBOARDING_RECORD_CHANNEL,
    ORCAROUTER_CHANNEL,
    PROVIDER_SETTINGS_CHANNEL,
    PLUGIN_MANAGEMENT_CHANNEL,
    GIT_CHECKPOINT_CHANNEL, HOOKS_CHANNEL, LAN_ACCESS_CHANNEL, MCP_SYNC_CHANNEL,
    MEDIA_PREVIEW_CHANNEL,
    MEMORY_CHANNEL, MODE_AGENT_CHANNEL, MODE_TASK_CHANNEL, PLUGIN_SYNC_CHANNEL, SETTING_CHANNEL,
    SETTINGS_SYNC_CHANNEL, SKILLS_CHANNEL, SKILL_SYNC_CHANNEL, SUBAGENTS_CHANNEL, SYSTEM_CHANNEL,
    TERMINAL_CHANNEL, USAGE_STATS_CHANNEL, WINDOW_CONTROLLER_CHANNEL,
};
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
    /// 进程级任务事件总线：per-window 频道的 mode-task/window-controller 与
    /// automation 宿主共用同一通知点（终态事件不按窗口分叉）。
    task_events: StdArc<host_channels::mode_task::TaskEventBus>,
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
            | MODEL_SELECTION_CHANNEL
            | MODE_SESSION_CHANNEL
            | ONBOARDING_RECORD_CHANNEL
            | ORCAROUTER_CHANNEL
            | PROVIDER_SETTINGS_CHANNEL
            | PLUGIN_MANAGEMENT_CHANNEL
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
            | MODE_TASK_CHANNEL
            | WINDOW_CONTROLLER_CHANNEL
            | LAN_ACCESS_CHANNEL
            | MCP_SYNC_CHANNEL
            | AGENT_HEALTH_CHANNEL
            | MODE_AGENT_CHANNEL
            | HOOKS_CHANNEL
            | TERMINAL_CHANNEL
    )
}

/// `default` 特值展开的频道集：全部白名单业务频道，含 `mode-agent`
/// （S2 翻面已随 W6 收口并入，2026-11-21）。`host` 永不在内（看门狗不能测壳自己）。
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
        MODEL_SELECTION_CHANNEL,
        MODE_SESSION_CHANNEL,
        ONBOARDING_RECORD_CHANNEL,
        ORCAROUTER_CHANNEL,
        PROVIDER_SETTINGS_CHANNEL,
        PLUGIN_MANAGEMENT_CHANNEL,
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
        MODE_TASK_CHANNEL,
        WINDOW_CONTROLLER_CHANNEL,
        LAN_ACCESS_CHANNEL,
        MCP_SYNC_CHANNEL,
        AGENT_HEALTH_CHANNEL,
        HOOKS_CHANNEL,
        TERMINAL_CHANNEL,
        MODE_AGENT_CHANNEL,
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
// 翻转点（W6 收口，spec w2-transport-cutover.md §5）：**已翻转（2026-11-21，用户
// 指令）**——「未设置」= 默认集（含 mode-agent）。真机验收清单
// `.plan/accept-w6/w2-flip-checklist.md`（七项）与
// `s2-mode-agent-checklist.md`（二十任务）转为翻转后的回归验证清单，逐项结果
// 回填 `.plan/accept-w6/report-<版本>.md`。回滚 = `MODE_HOST_INPROCESS_CHANNELS=none`
// （纯转发，回滚语义最强，出现即忽略其余项）。
fn resolve_channel_requests(raw: Option<&str>) -> Vec<String> {
    let Some(raw) = raw else {
        // W2 翻面：默认集接管全部白名单业务频道。
        let mut owned = default_owned_channels();
        // S2 翻面：mode-agent 整体切换（spec host-ownership-switch.md，无半个频道）。
        owned.push(MODE_AGENT_CHANNEL.to_string());
        return owned;
    };
    let requested: Vec<String> = raw
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
        mode_builtin_provider_config_path: Option<String>,
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
        let task_events = StdArc::new(host_channels::mode_task::TaskEventBus::new());

        if enabled {
            let (out_tx, mut out_rx) = unbounded_channel::<Vec<u8>>();
            tauri::async_runtime::spawn(run_consumer(
                Arc::clone(&supervisor),
                owned.clone(),
                data_root,
                Arc::clone(&agent_control),
                StdArc::clone(&task_events),
                mode_builtin_provider_config_path.clone(),
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
            task_events,
        })
    }

    pub fn agent_control(&self) -> &StdArc<AgentControlClient> {
        &self.agent_control
    }

    pub fn task_events(&self) -> &StdArc<host_channels::mode_task::TaskEventBus> {
        &self.task_events
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

// 消费循环是频道装配的唯一参数汇聚点（与 build_server 同因）。
#[allow(clippy::too_many_arguments)]
async fn run_consumer(
    supervisor: Arc<HostSupervisor>,
    owned: Vec<String>,
    data_root: PathBuf,
    agent_control: StdArc<AgentControlClient>,
    task_events: StdArc<host_channels::mode_task::TaskEventBus>,
    mode_builtin_provider_config_path: Option<String>,
    mut rx: UnboundedReceiver<(Option<u32>, Vec<u8>)>,
    out_tx: UnboundedSender<Vec<u8>>,
) {
    let mut generation = supervisor.current().startup_id().to_string();
    let mut base = build_server(
        out_tx.clone(),
        &owned,
        &data_root,
        &agent_control,
        &task_events,
        mode_builtin_provider_config_path.as_deref(),
    );
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
            base = build_server(
                out_tx.clone(),
                &owned,
                &data_root,
                &agent_control,
                &task_events,
                mode_builtin_provider_config_path.as_deref(),
            );
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
                        &task_events,
                        mode_builtin_provider_config_path.as_deref(),
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

// 频道装配参数随已迁移频道增长；保持单一构造点比拆结构体更不易漏传（同 automation
// runtime 的 too_many_arguments 处理）。
#[allow(clippy::too_many_arguments)]
fn build_server(
    out_tx: UnboundedSender<Vec<u8>>,
    owned: &[String],
    data_root: &Path,
    agent_control: &StdArc<AgentControlClient>,
    task_events: &StdArc<host_channels::mode_task::TaskEventBus>,
    mode_builtin_provider_config_path: Option<&str>,
) -> ChannelServer {
    let mut server = ChannelServer::new(
        out_tx,
        "mode-host-sidecar",
        Duration::from_millis(1000),
        true,
    );
    // 频道构造清单抽到 `host_channels::assembly`（LT-15 P2 切一）供 server-mode
    // 复用；分支体逐行等价搬移，壳日志注入化。mode-agent 的注册与否仍由
    // `owned` 名单决定（S2 门禁不变）。
    let options = host_channels::ChannelAssemblyOptions {
        data_root: data_root.to_path_buf(),
        agent_control: Some(StdArc::clone(agent_control)),
        on_shell_log: Some(StdArc::new(|message: &str| crate::host::shell_log(message))),
        // 进程级共享：mode-task → window-controller 事件桥与 automation 终态订阅同源。
        task_events: StdArc::clone(task_events),
        // LAN supervisor 装配待 inprocess 接入（16.4a 续）：当前 lan-access 管理面显式报错。
        lan_host: None,
        mode_builtin_provider_config_path: mode_builtin_provider_config_path
            .map(str::to_string),
    };
    for name in owned {
        match build_channel(name, &options) {
            Some(channel) => server.register_channel(name, channel),
            None => {
                crate::host::shell_log(&format!("inprocess channel not constructible: {name}"))
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
        // mode-task 已进壳（LT-16.1，2026-11-22）；仍在 Node Host 的独占面：
        // window-controller 与 remote 系列等。
        assert!(is_owned_channel(MODE_TASK_CHANNEL));
        assert!(is_owned_channel(WINDOW_CONTROLLER_CHANNEL));
        assert!(!is_owned_channel("host"));
    }

    #[test]
    fn unset_switch_takes_the_default_set_with_mode_agent() {
        // W2+S2 已翻转（2026-11-21）：未设置 = 默认集（含 mode-agent）；
        // 回滚 = 显式 `none`。
        let resolved = resolve_channel_requests(None);
        assert!(!resolved.is_empty());
        assert!(resolved.iter().any(|name| name == MODE_AGENT_CHANNEL));
        assert!(resolved.iter().any(|name| name == MEMORY_CHANNEL));
    }

    #[test]
    fn none_wins_and_disables_everything() {
        // 显式回滚语义最强：与其它项混用时以 none 为准。
        assert!(resolve_channel_requests(Some("none")).is_empty());
        assert!(resolve_channel_requests(Some("memory, none ,git")).is_empty());
    }

    #[test]
    fn default_set_matches_the_shared_service_channel_list() {
        // 两份名单（壳内 inprocess 与 assembly::service_channel_names）必须同步：
        // 漏一边 = 「桌面端接管了、server-mode 没接管」或反之，且两边都不会报错，
        // 只会表现为某个环境里频道静默回落 Node。这里用集合相等把它钉住。
        let owned = resolve_channel_requests(None);
        let mut shell: Vec<&str> = owned.iter().map(|name| name.as_str()).collect();
        let mut shared: Vec<&str> = host_channels::service_channel_names();
        // assembly 的名单按契约不含 mode-agent（S2 门禁由调用方持有）。
        shared.push(MODE_AGENT_CHANNEL);
        shell.sort_unstable();
        shell.dedup();
        shared.sort_unstable();
        shared.dedup();
        assert_eq!(
            shell, shared,
            "inprocess 默认集与 assembly::service_channel_names 不一致"
        );
    }

    #[test]
    fn default_expands_with_mode_agent_but_never_host() {
        let resolved = resolve_channel_requests(Some("default"));
        assert!(!resolved.is_empty());
        // S2 翻面后 default 含 mode-agent；host 永不在内（看门狗不能测壳自己）。
        assert!(resolved.iter().any(|name| name == MODE_AGENT_CHANNEL));
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
