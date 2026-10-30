//! v4 传输面：`v4/conversation/subscribe` 与其 wire frame 外壳。
//!
//! 真源：`packages/shared/src/mode-protocol-v4/transport.ts` 的
//! `v4ConversationSubscribeParamsSchema` / `v4ConversationSubscribeResultSchema`，
//! 以及 `wire.ts` 的 `createTopicWireFrameSchema`。
//!
//! **范围**：本模块严格镜像订阅参数、ACK 与 wire 外壳；`frame` 里的会话快照/增量
//! （`conversationSnapshotSchema` / `conversationDeltaSchema`）体量巨大，暂按原始 JSON
//! 透传，等独立切片再建模。规范见 `docs/specs/session-write-group.md` §「v4 会话订阅」。

use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

use crate::session_read::SessionWorkspaceRef;
use crate::v4_delta::ConversationDelta;
use crate::v4_snapshot::ConversationSnapshot;
use crate::workflow::LiteralTrue;

pub const V4_CONVERSATION_SUBSCRIBE_METHOD: &str = "v4/conversation/subscribe";
pub const V4_CONVERSATION_FRAME_NOTIFICATION: &str = "v4/conversation/frame";
pub const V4_WIRE_PROTOCOL_VERSION: u64 = 3;
pub const MAX_LEGACY_TASK_IDS_PER_SUBSCRIBE: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum V4ClientMode {
    #[serde(rename = "desktop-continuous")]
    DesktopContinuous,
    #[serde(rename = "web-remote-replayable")]
    WebRemoteReplayable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubscribeBase {
    pub log_epoch: String,
    pub seq: Number,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubscribeVisibility {
    #[serde(rename = "foreground")]
    Foreground,
    #[serde(rename = "background")]
    Background,
}

/// `v4ConversationSubscribeParamsSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationSubscribeParams {
    pub topic: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<SubscribeBase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<SubscribeVisibility>,
    pub connection_id: String,
    pub client_mode: V4ClientMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<SessionWorkspaceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_task_ids: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_thought_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_run_deltas: Option<bool>,
}

impl ConversationSubscribeParams {
    /// `legacyTaskIds` 上限 200（协议共享窗口）。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self
            .legacy_task_ids
            .as_ref()
            .is_some_and(|ids| ids.len() > MAX_LEGACY_TASK_IDS_PER_SUBSCRIBE)
        {
            return Err("legacyTaskIds exceeds 200 entries");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubscribeMode {
    #[serde(rename = "snapshot")]
    Snapshot,
    #[serde(rename = "resume")]
    Resume,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CliProcessState {
    #[serde(rename = "spawned")]
    Spawned,
    #[serde(rename = "reused")]
    Reused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionRuntimeState {
    #[serde(rename = "cold")]
    Cold,
    #[serde(rename = "warm")]
    Warm,
}

/// `conversationOpenTimingSchema`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationOpenTiming {
    pub version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_prepare_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_registry_sync_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_meta_read_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_request_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_bootstrap_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_session_restore_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_frame_encode_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_process_state: Option<CliProcessState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_runtime_state: Option<SessionRuntimeState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_row_count: Option<u64>,
}

/// `conversationSubscribeAckSchema`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationSubscribeAck {
    pub subscription_id: String,
    pub mode: SubscribeMode,
    pub log_epoch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_timing: Option<ConversationOpenTiming>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationSubscribeResult {
    pub ack: ConversationSubscribeAck,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireDeliveryKind {
    #[serde(rename = "initial")]
    Initial,
    #[serde(rename = "online")]
    Online,
    #[serde(rename = "recovery")]
    Recovery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChecksumAlgorithm {
    #[serde(rename = "crc32")]
    Crc32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TopicWireChecksum {
    pub algorithm: ChecksumAlgorithm,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireFrameKindComplete {
    #[serde(rename = "complete")]
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireFrameKindFragment {
    #[serde(rename = "fragment")]
    Fragment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FramePayloadKindSnapshot {
    #[serde(rename = "snapshot")]
    Snapshot,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SnapshotFramePayload {
    pub kind: FramePayloadKindSnapshot,
    pub snapshot: ConversationSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FramePayloadKindDeltas {
    #[serde(rename = "deltas")]
    Deltas,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeltasFramePayload {
    pub kind: FramePayloadKindDeltas,
    pub deltas: Vec<ConversationDelta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConversationFramePayload {
    Snapshot(Box<SnapshotFramePayload>),
    Deltas(DeltasFramePayload),
}

/// `conversationTopicFrameSchema`；`ttft` / `ttftRelated` 暂按原始 JSON。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationTopicFrame {
    pub topic: String,
    pub subscription_id: String,
    pub from_seq: Number,
    pub to_seq: Number,
    pub sent_at: Number,
    pub payload: ConversationFramePayload,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttft: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttft_related: Option<Vec<Value>>,
}

/// 完整 wire frame：`frame` 是强类型的会话快照/增量。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompleteWireFrame {
    pub wire_version: u64,
    pub kind: WireFrameKindComplete,
    pub delivery_kind: WireDeliveryKind,
    pub logical_frame_id: String,
    pub logical_frame_ordinal: u64,
    pub topic: String,
    pub subscription_id: String,
    pub frame: ConversationTopicFrame,
}

/// 分片 wire frame（host 侧只做路由与集合，重组属于消费方）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FragmentWireFrame {
    pub wire_version: u64,
    pub kind: WireFrameKindFragment,
    pub delivery_kind: WireDeliveryKind,
    pub logical_frame_id: String,
    pub logical_frame_ordinal: u64,
    pub topic: String,
    pub subscription_id: String,
    pub fragment_index: u64,
    pub fragment_count: u64,
    pub logical_bytes: u64,
    pub checksum: TopicWireChecksum,
    pub data_base64: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConversationWireFrame {
    Complete(Box<CompleteWireFrame>),
    Fragment(FragmentWireFrame),
}

impl ConversationWireFrame {
    pub fn subscription_id(&self) -> &str {
        match self {
            Self::Complete(frame) => &frame.subscription_id,
            Self::Fragment(frame) => &frame.subscription_id,
        }
    }
}

/// `workflowRunDeltas` 声明位：与 clientMode 同族的可信位（由连接注入）。
pub type WorkflowRunDeltas = LiteralTrue;

pub const V4_HOST_HELLO_KIND: &str = "hello";
pub const V4_CLIENT_HELLO_KIND: &str = "clientHello";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Compression {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "permessage-deflate")]
    PermessageDeflate,
}

/// `hostCapabilitiesSchema`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostCapabilities {
    pub native_dialogs: bool,
    pub local_terminal: bool,
    pub binary_frames: bool,
    pub compression: Compression,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_hook_review: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub independent_plan_state: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_run_deltas: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeliveryProfile {
    #[serde(rename = "continuous")]
    Continuous,
    #[serde(rename = "replayable")]
    Replayable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HelloAuth {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HelloKind {
    #[serde(rename = "hello")]
    Hello,
}

/// `helloMessageSchema`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HelloMessage {
    pub kind: HelloKind,
    pub protocol_version: u64,
    pub connection_id: String,
    pub client_mode: V4ClientMode,
    pub delivery_profile: DeliveryProfile,
    pub server_time: Number,
    pub capabilities: HostCapabilities,
    pub auth: HelloAuth,
}

impl HelloMessage {
    /// `superRefine`：desktop-continuous ↔ continuous，其余 ↔ replayable。
    pub fn validate(&self) -> Result<(), &'static str> {
        let expected = match self.client_mode {
            V4ClientMode::DesktopContinuous => DeliveryProfile::Continuous,
            V4ClientMode::WebRemoteReplayable => DeliveryProfile::Replayable,
        };
        if self.delivery_profile != expected {
            return Err("trusted clientMode and deliveryProfile must match");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientKind {
    #[serde(rename = "desktop")]
    Desktop,
    #[serde(rename = "web")]
    Web,
    #[serde(rename = "mobileRemote")]
    MobileRemote,
    #[serde(rename = "mobileApp")]
    MobileApp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_hook_review_ui: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_run_deltas: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientHelloKind {
    #[serde(rename = "clientHello")]
    ClientHello,
}

/// `clientHelloSchema`（`.strict()`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientHello {
    pub kind: ClientHelloKind,
    pub protocol_version: u64,
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_kind: Option<ClientKind>,
    pub app_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<ClientCapabilities>,
}
