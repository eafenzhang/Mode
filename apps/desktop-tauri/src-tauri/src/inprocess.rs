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
//! 开关：`MODE_HOST_INPROCESS_CHANNELS=<逗号分隔频道>`；为空时整条链路完全不介入。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use host_channels::{
    AttachmentTransferChannel, ClientConfigChannel, ClientScenesChannel, ConversationMirrorChannel,
    CredentialChannel, MediaPreviewChannel, MemoryChannel, SettingChannel, SystemChannel,
    ATTACHMENT_TRANSFER_CHANNEL, CLIENT_CONFIG_CHANNEL, CLIENT_SCENES_CHANNEL,
    CONVERSATION_MIRROR_CHANNEL, CREDENTIAL_CHANNEL, MEDIA_PREVIEW_CHANNEL, MEMORY_CHANNEL,
    SETTING_CHANNEL, SYSTEM_CHANNEL,
};
use project_memory::ProjectMemoryService;
use rpc_host::{ChannelServer, FrameRoute, FrameRouter, LocalScopeRegistry};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::host::HostSupervisor;

const ENV_CHANNELS: &str = "MODE_HOST_INPROCESS_CHANNELS";

/// 一帧的去向。
pub enum RouteDecision {
    /// 本地服务面处理；带上剥掉 scope 前缀后的原始 payload 与所属 scope。
    Local { scope_id: Option<u32>, payload: Vec<u8> },
    /// 已判定为不可处理（畸形帧/非请求）：丢弃，不回帧也不转发。
    Dropped,
    /// 交给遗留 Node Host（逐字节转发）。
    Forward,
}

pub struct InprocessState {
    router: FrameRouter,
    local_tx: UnboundedSender<(Option<u32>, Vec<u8>)>,
    enabled: bool,
    /// 页面桥声明挂接/回收的 remote scope。服务面目前只接管 base scope；这张表先如实
    /// 记录，供下一步按 scope 复用 `ChannelServer` 与回帧加前缀时使用。
    scopes: std::sync::Mutex<LocalScopeRegistry>,
}

impl InprocessState {
    /// 读开关并起消费/回帧两条任务。开关为空时不注册任何频道，链路退化为纯转发。
    pub fn from_env(supervisor: Arc<HostSupervisor>, data_root: PathBuf) -> Arc<Self> {
        let requested: Vec<String> = std::env::var(ENV_CHANNELS)
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect();

        let mut owned = Vec::new();
        for name in requested {
            match name.as_str() {
                MEMORY_CHANNEL
                | ATTACHMENT_TRANSFER_CHANNEL
                | CLIENT_CONFIG_CHANNEL
                | CLIENT_SCENES_CHANNEL
                | MEDIA_PREVIEW_CHANNEL
                | CONVERSATION_MIRROR_CHANNEL
                | SETTING_CHANNEL
                | CREDENTIAL_CHANNEL
                | SYSTEM_CHANNEL => owned.push(name),
                // 还没移植的频道明确忽略：宁可不接管，也不能出现「半个频道」。
                other => crate::host::shell_log(&format!("inprocess channel ignored (not ported yet): {other}")),
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
                        Err(error) => {
                            crate::host::shell_log(&format!("inprocess forward task failed: {error}"))
                        }
                    }
                }
            });
            // release 是 windows_subsystem="windows"：eprintln 没有 stderr 落地，
            // 这条诊断必须走壳侧结构化日志，否则「开没开、丢了帧」都不可观察。
            crate::host::shell_log(&format!(
                "inprocess channels enabled: {}",
                owned.join(",")
            ));
        }

        Arc::new(Self {
            router,
            local_tx,
            enabled,
            scopes: std::sync::Mutex::new(LocalScopeRegistry::new()),
        })
    }

    pub fn decide(&self, frame: &[u8]) -> RouteDecision {
        if !self.enabled {
            return RouteDecision::Forward;
        }
        match self.router.route(frame) {
            FrameRoute::Local {
                payload, scope_id, ..
            } => RouteDecision::Local { scope_id, payload },
            FrameRoute::Rejected => RouteDecision::Dropped,
            FrameRoute::Control(control) => {
                // 控制帧仍要转发给 Node（未移植频道的 scope 生命周期归它），
                // 这里先把挂接状态同步到壳内登记表。
                if let Ok(mut scopes) = self.scopes.lock() {
                    match control {
                        rpc_wire::AttachmentControl::ScopeReady(scope_id) => {
                            scopes.attach(scope_id);
                        }
                        rpc_wire::AttachmentControl::ScopeDetach(scope_id) => {
                            scopes.detach(scope_id);
                        }
                    }
                }
                RouteDecision::Forward
            }
            FrameRoute::Legacy => RouteDecision::Forward,
        }
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
    mut rx: UnboundedReceiver<(Option<u32>, Vec<u8>)>,
    out_tx: UnboundedSender<Vec<u8>>,
) {
    let mut generation = supervisor.current().startup_id().to_string();
    let mut base = build_server(out_tx.clone(), &owned, &data_root);
    let mut scoped: std::collections::HashMap<u32, ChannelServer> = std::collections::HashMap::new();
    while let Some((scope_id, payload)) = rx.recv().await {
        let current = supervisor.current().startup_id().to_string();
        if current != generation {
            // 换代：对端 id 空间从 0 重新计数，base 与各 scope 的 active 请求都必须释放。
            base.dispose();
            for (_, mut server) in scoped.drain() {
                server.dispose();
            }
            base = build_server(out_tx.clone(), &owned, &data_root);
            generation = current;
        }
        match scope_id {
            None => base.handle_frame(&payload),
            Some(scope) => {
                let server = scoped
                    .entry(scope)
                    .or_insert_with(|| build_server(scoped_out(&out_tx, scope), &owned, &data_root));
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
) -> ChannelServer {
    let mut server = ChannelServer::new(out_tx, "mode-host-sidecar", Duration::from_millis(1000), true);
    for name in owned {
        match name.as_str() {
            MEMORY_CHANNEL => server.register_channel(
                MEMORY_CHANNEL,
                Arc::new(MemoryChannel::new(Arc::new(ProjectMemoryService::new(
                    data_root.to_path_buf(),
                )))),
            ),
            ATTACHMENT_TRANSFER_CHANNEL => server.register_channel(
                ATTACHMENT_TRANSFER_CHANNEL,
                Arc::new(AttachmentTransferChannel),
            ),
            CLIENT_CONFIG_CHANNEL => server.register_channel(
                CLIENT_CONFIG_CHANNEL,
                Arc::new(ClientConfigChannel),
            ),
            CLIENT_SCENES_CHANNEL => server.register_channel(
                CLIENT_SCENES_CHANNEL,
                Arc::new(ClientScenesChannel),
            ),
            MEDIA_PREVIEW_CHANNEL => server.register_channel(
                MEDIA_PREVIEW_CHANNEL,
                Arc::new(MediaPreviewChannel),
            ),
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
                let store = credential_store::CredentialStore::new(data_root.to_path_buf())
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
                Arc::new(SystemChannel::new(Arc::new(system_service::SystemService::new()))),
            ),
            other => crate::host::shell_log(&format!("inprocess channel not constructible: {other}")),
        }
    }
    server
}
