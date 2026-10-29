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
    AttachmentTransferChannel, ClientConfigChannel, ClientScenesChannel, MediaPreviewChannel,
    MemoryChannel, ATTACHMENT_TRANSFER_CHANNEL, CLIENT_CONFIG_CHANNEL, CLIENT_SCENES_CHANNEL,
    MEDIA_PREVIEW_CHANNEL, MEMORY_CHANNEL,
};
use project_memory::ProjectMemoryService;
use rpc_host::{ChannelServer, FrameRoute, FrameRouter};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::host::HostSupervisor;

const ENV_CHANNELS: &str = "MODE_HOST_INPROCESS_CHANNELS";

/// 一帧的去向。
pub enum RouteDecision {
    /// 本地服务面处理；带上剥掉 scope 前缀后的原始 payload。
    Local(Vec<u8>),
    /// 已判定为不可处理（畸形帧/非请求）：丢弃，不回帧也不转发。
    Dropped,
    /// 交给遗留 Node Host（逐字节转发）。
    Forward,
}

pub struct InprocessState {
    router: FrameRouter,
    local_tx: UnboundedSender<Vec<u8>>,
    enabled: bool,
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
                | MEDIA_PREVIEW_CHANNEL => owned.push(name),
                // 还没移植的频道明确忽略：宁可不接管，也不能出现「半个频道」。
                other => eprintln!("[mode-tauri] inprocess channel ignored (not ported yet): {other}"),
            }
        }
        let enabled = !owned.is_empty();
        let router = FrameRouter::base_scope_only(owned.iter().cloned());
        let (local_tx, local_rx) = unbounded_channel::<Vec<u8>>();

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
                            eprintln!("[mode-tauri] inprocess frame dropped: {error}")
                        }
                        Err(error) => {
                            eprintln!("[mode-tauri] inprocess forward task failed: {error}")
                        }
                    }
                }
            });
            eprintln!(
                "[mode-tauri] inprocess channels enabled: {}",
                owned.join(",")
            );
        }

        Arc::new(Self {
            router,
            local_tx,
            enabled,
        })
    }

    pub fn decide(&self, frame: &[u8]) -> RouteDecision {
        if !self.enabled {
            return RouteDecision::Forward;
        }
        match self.router.route(frame) {
            FrameRoute::Local { payload, .. } => RouteDecision::Local(payload),
            FrameRoute::Rejected => RouteDecision::Dropped,
            FrameRoute::Control(_) | FrameRoute::Legacy => RouteDecision::Forward,
        }
    }

    pub fn dispatch(&self, payload: Vec<u8>) {
        if self.local_tx.send(payload).is_err() {
            eprintln!("[mode-tauri] inprocess consumer is gone; frame dropped");
        }
    }
}

async fn run_consumer(
    supervisor: Arc<HostSupervisor>,
    owned: Vec<String>,
    data_root: PathBuf,
    mut rx: UnboundedReceiver<Vec<u8>>,
    out_tx: UnboundedSender<Vec<u8>>,
) {
    let mut generation = supervisor.current().startup_id().to_string();
    let mut server = build_server(out_tx.clone(), &owned, &data_root);
    while let Some(payload) = rx.recv().await {
        let current = supervisor.current().startup_id().to_string();
        if current != generation {
            // 换代：对端 id 空间从 0 重新计数，旧 active 请求/订阅必须全部释放。
            server.dispose();
            server = build_server(out_tx.clone(), &owned, &data_root);
            generation = current;
        }
        server.handle_frame(&payload);
    }
    server.dispose();
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
            other => eprintln!("[mode-tauri] inprocess channel not constructible: {other}"),
        }
    }
    server
}
