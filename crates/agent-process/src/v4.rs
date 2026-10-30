//! v4 面：`resolveInteraction` 命令闭环与只读查询（`rowsRange` / `plans`）。
//!
//! 行为真源：`packages/services/src/mode-agent/modeV4HostCommand.ts` 的
//! `createHostCommandEnvelope` / `assertV4CommandAckOk`，以及
//! `modeAgentService.sendConversationCommandV4` 的调用形状。
//!
//! 差异（已写入 spec）：
//! - `commandId` / `clientId` 只保证「非空且按提交端稳定」，不是 UUID v7（协议 schema 只要求
//!   字符串；幂等表以 `commandId` 为键，语义不受影响）。
//! - 订阅/信任连接（`helloConversationV4`）尚未迁移；`resolveInteraction` 不依赖它。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_protocol::v4::{
    CommandAck, ResolveInteractionAnswer, ResolveInteractionEnvelope, ResolveInteractionPayload,
    ResolveInteractionType, V4_COMMAND_METHOD,
};
use agent_protocol::v4_transport::{
    ClientHello, Compression, DeliveryProfile, HelloAuth, HelloKind, HelloMessage,
    HostCapabilities, V4ClientMode, V4_WIRE_PROTOCOL_VERSION,
};
use agent_protocol::v4_queries::{
    AppUsageSnapshot, AttachmentAbortResult, AttachmentBeginParams, AttachmentBeginResult,
    AttachmentChunkParams,
    AttachmentChunkResult, AttachmentCommitResult, AttachmentPreviewSourceParams,
    AttachmentPreviewSourceResult, AttachmentReadParams, AttachmentReadResult,
    AttachmentTerminalParams, BackgroundBashOutputParams, BackgroundBashOutputResult,
    BackgroundBashUnavailable, BackgroundBashUnavailableKind, ConnectionFlowParams,
    ConnectionFlowResult, ConversationAttachmentReadParams, ConversationAttachmentStatParams,
    ConversationAttachmentStatResult, ConversationResyncParams, ConversationResyncResult,
    ConversationUnsubscribeParams, ConversationUsageParams, ConversationUsageResult,
    FileChangesParams, FileChangesResult, FileRewindPreviewParams, FileRewindPreviewResult,
    PlansParams, PlansResult, RowsRangeParams, RowsRangeResult, WorkflowRunArtifactDataParams,
    WorkflowRunArtifactDataResult, WorkflowRunArtifactReadParams, WorkflowRunArtifactReadResult,
    WorkflowRunArtifactsParams, WorkflowRunArtifactsResult, WorkflowRunEventsParams,
    WorkflowRunEventsResult, WorkflowRunNodeResult, WorkflowRunNodeResultParams,
    WorkflowRunWorkspaceParams, WorkflowRunWorkspaceResult, WorkflowRunsQueryParams,
    WorkflowRunsQueryResult, UsageStatsParams, V4_USAGE_STATS_METHOD, V4_ATTACHMENT_ABORT_METHOD, V4_ATTACHMENT_BEGIN_METHOD,
    V4_ATTACHMENT_CHUNK_METHOD, V4_ATTACHMENT_COMMIT_METHOD,
    V4_ATTACHMENT_PREVIEW_SOURCE_METHOD, V4_ATTACHMENT_READ_METHOD,
    V4_BACKGROUND_BASH_OUTPUT_METHOD, V4_CONNECTION_FLOW_METHOD,
    V4_CONVERSATION_ATTACHMENT_READ_METHOD,
    V4_CONVERSATION_ATTACHMENT_STAT_METHOD, V4_CONVERSATION_FILE_CHANGES_METHOD,
    V4_CONVERSATION_FILE_REWIND_PREVIEW_METHOD, V4_CONVERSATION_PLANS_METHOD,
    V4_CONVERSATION_RESYNC_METHOD, V4_CONVERSATION_ROWS_RANGE_METHOD,
    V4_CONVERSATION_UNSUBSCRIBE_METHOD, V4_CONVERSATION_USAGE_METHOD,
    V4_CONVERSATION_WORKFLOW_RUN_ARTIFACT_DATA_METHOD,
    V4_CONVERSATION_WORKFLOW_RUN_ARTIFACT_READ_METHOD,
    V4_CONVERSATION_WORKFLOW_RUN_ARTIFACTS_METHOD, V4_CONVERSATION_WORKFLOW_RUN_EVENTS_METHOD,
    V4_CONVERSATION_WORKFLOW_RUN_NODE_RESULT_METHOD, V4_CONVERSATION_WORKFLOW_RUN_WORKSPACE_METHOD,
    V4_CONVERSATION_WORKFLOW_RUNS_METHOD,
};
use mode_protocol_client::ClientError;
use serde_json::Value;

use crate::control::{AgentControlClient, AgentWorkspaceTarget, ControlError, RuntimePolicy};

static COMMAND_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// host 的 hello（与 TS `helloConversationV4` 的能力集一致）。
pub fn build_host_hello(connection_id: &str) -> HelloMessage {
    HelloMessage {
        kind: HelloKind::Hello,
        protocol_version: V4_WIRE_PROTOCOL_VERSION,
        connection_id: connection_id.to_string(),
        client_mode: V4ClientMode::DesktopContinuous,
        delivery_profile: DeliveryProfile::Continuous,
        server_time: serde_json::Number::from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_millis() as u64)
                .unwrap_or(0),
        ),
        capabilities: HostCapabilities {
            native_dialogs: true,
            local_terminal: true,
            binary_frames: false,
            compression: Compression::None,
            workspace_hook_review: Some(true),
            independent_plan_state: Some(true),
            workflow_run_deltas: Some(true),
        },
        auth: HelloAuth { user_id: None },
    }
}

/// `initializeConversationV4`：严格解析客户端 hello（未知键拒绝）。
pub fn validate_client_hello(value: &Value) -> Result<ClientHello, ControlError> {
    serde_json::from_value(value.clone())
        .map_err(|error| ControlError::InvalidResult(format!("invalid clientHello: {error}")))
}

fn next_command_id() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let sequence = COMMAND_SEQUENCE.fetch_add(1, Ordering::SeqCst);
    format!("{now:x}-{:x}-{sequence:x}", std::process::id())
}

impl AgentControlClient {
    /// 回执一次交互请求（权限或 AskUserQuestion）。`accepted` / `duplicate` / `noop`
    /// 按幂等成功返回；`rejected` / `stale` / `failed` 返回 `CommandRejected`。
    pub async fn resolve_interaction_v4(
        &self,
        target: &AgentWorkspaceTarget,
        session_id: &str,
        interaction_id: &str,
        answer: ResolveInteractionAnswer,
    ) -> Result<CommandAck, ControlError> {
        let process = self.client_for(target, RuntimePolicy::StartIfNeeded).await?;
        let envelope = ResolveInteractionEnvelope {
            command_id: next_command_id(),
            client_id: self.v4_client_id().to_string(),
            session_id: session_id.to_string(),
            command_type: ResolveInteractionType::ResolveInteraction,
            payload: ResolveInteractionPayload {
                interaction_id: interaction_id.to_string(),
                answer,
            },
            issued_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_millis() as u64)
                .unwrap_or(0),
        };
        let params = serde_json::to_value(&envelope).expect("v4/command params 序列化不会失败");
        let response = process
            .client()
            .request(V4_COMMAND_METHOD, Some(params))
            .await?;
        let ack: CommandAck = serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!("invalid v4/command result: {error}"))
        })?;
        if !ack.status.is_success() {
            let mut message = format!("v4 command resolveInteraction {}", ack.status.as_str());
            if let Some(reason) = &ack.reason_code {
                message.push_str(&format!(" ({reason})"));
            }
            if let Some(detail) = &ack.message {
                message.push_str(&format!(": {detail}"));
            }
            return Err(ControlError::CommandRejected(message));
        }
        Ok(ack)
    }

    /// `v4/conversation/rowsRange`：游标制行分页（loadOlder）。只读、无状态，
    /// 陈旧读防护由调用方按 `atLogEpoch` 判断。
    pub async fn conversation_rows_range_v4(
        &self,
        target: &AgentWorkspaceTarget,
        params: RowsRangeParams,
        policy: RuntimePolicy,
    ) -> Result<RowsRangeResult, ControlError> {
        let process = self.client_for(target, policy).await?;
        let value = serde_json::to_value(&params).expect("rowsRange params 序列化不会失败");
        let response = process
            .client()
            .request(V4_CONVERSATION_ROWS_RANGE_METHOD, Some(value))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!("invalid v4/conversation/rowsRange result: {error}"))
        })
    }

    /// `v4/conversation/plans`：计划目录来自 CLI 完整 projection，不用 renderer 的有界窗口
    /// 推导。
    pub async fn conversation_plans_v4(
        &self,
        target: &AgentWorkspaceTarget,
        params: PlansParams,
        policy: RuntimePolicy,
    ) -> Result<PlansResult, ControlError> {
        let process = self.client_for(target, policy).await?;
        let value = serde_json::to_value(&params).expect("plans params 序列化不会失败");
        let response = process
            .client()
            .request(V4_CONVERSATION_PLANS_METHOD, Some(value))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!("invalid v4/conversation/plans result: {error}"))
        })
    }

    /// `v4/conversation/workflowRunEvents`：journal 分页；分页在存储层完成，RPC 层不切片。
    pub async fn conversation_workflow_run_events_v4(
        &self,
        target: &AgentWorkspaceTarget,
        params: WorkflowRunEventsParams,
        policy: RuntimePolicy,
    ) -> Result<WorkflowRunEventsResult, ControlError> {
        let process = self.client_for(target, policy).await?;
        let value = serde_json::to_value(&params).expect("workflowRunEvents params 序列化不会失败");
        let response = process
            .client()
            .request(V4_CONVERSATION_WORKFLOW_RUN_EVENTS_METHOD, Some(value))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!(
                "invalid v4/conversation/workflowRunEvents result: {error}"
            ))
        })
    }

    /// `v4/conversation/workflowRuns`：journal-backed 的 run 枚举（重启后发现面）。
    pub async fn conversation_workflow_runs_v4(
        &self,
        target: &AgentWorkspaceTarget,
        params: WorkflowRunsQueryParams,
        policy: RuntimePolicy,
    ) -> Result<WorkflowRunsQueryResult, ControlError> {
        let process = self.client_for(target, policy).await?;
        let value = serde_json::to_value(&params).expect("workflowRuns params 序列化不会失败");
        let response = process
            .client()
            .request(V4_CONVERSATION_WORKFLOW_RUNS_METHOD, Some(value))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!(
                "invalid v4/conversation/workflowRuns result: {error}"
            ))
        })
    }

    /// `v4/conversation/workflowRunArtifacts`：用户面产物清单（全量版本元数据）。
    pub async fn conversation_workflow_run_artifacts_v4(
        &self,
        target: &AgentWorkspaceTarget,
        params: WorkflowRunArtifactsParams,
        policy: RuntimePolicy,
    ) -> Result<WorkflowRunArtifactsResult, ControlError> {
        let process = self.client_for(target, policy).await?;
        let value = serde_json::to_value(&params).expect("workflowRunArtifacts params 序列化不会失败");
        let response = process
            .client()
            .request(V4_CONVERSATION_WORKFLOW_RUN_ARTIFACTS_METHOD, Some(value))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!(
                "invalid v4/conversation/workflowRunArtifacts result: {error}"
            ))
        })
    }

    /// `v4/conversation/workflowRunArtifactData`：预置看板的条目页（cursor = journal
    /// sequence）。
    pub async fn conversation_workflow_run_artifact_data_v4(
        &self,
        target: &AgentWorkspaceTarget,
        params: WorkflowRunArtifactDataParams,
        policy: RuntimePolicy,
    ) -> Result<WorkflowRunArtifactDataResult, ControlError> {
        let process = self.client_for(target, policy).await?;
        let value =
            serde_json::to_value(&params).expect("workflowRunArtifactData params 序列化不会失败");
        let response = process
            .client()
            .request(V4_CONVERSATION_WORKFLOW_RUN_ARTIFACT_DATA_METHOD, Some(value))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!(
                "invalid v4/conversation/workflowRunArtifactData result: {error}"
            ))
        })
    }

    /// `v4/conversation/workflowRunArtifactRead`：内容产物一次一块的字节。
    pub async fn conversation_workflow_run_artifact_read_v4(
        &self,
        target: &AgentWorkspaceTarget,
        params: WorkflowRunArtifactReadParams,
        policy: RuntimePolicy,
    ) -> Result<WorkflowRunArtifactReadResult, ControlError> {
        let process = self.client_for(target, policy).await?;
        let value =
            serde_json::to_value(&params).expect("workflowRunArtifactRead params 序列化不会失败");
        let response = process
            .client()
            .request(V4_CONVERSATION_WORKFLOW_RUN_ARTIFACT_READ_METHOD, Some(value))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!(
                "invalid v4/conversation/workflowRunArtifactRead result: {error}"
            ))
        })
    }

    /// `v4/conversation/workflowRunWorkspace`：工作区 transcript 的轻行清单（不带正文）。
    pub async fn conversation_workflow_run_workspace_v4(
        &self,
        target: &AgentWorkspaceTarget,
        params: WorkflowRunWorkspaceParams,
        policy: RuntimePolicy,
    ) -> Result<WorkflowRunWorkspaceResult, ControlError> {
        let process = self.client_for(target, policy).await?;
        let value =
            serde_json::to_value(&params).expect("workflowRunWorkspace params 序列化不会失败");
        let response = process
            .client()
            .request(V4_CONVERSATION_WORKFLOW_RUN_WORKSPACE_METHOD, Some(value))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!(
                "invalid v4/conversation/workflowRunWorkspace result: {error}"
            ))
        })
    }

    /// `v4/conversation/workflowRunNodeResult`：一个节点的正文（按 `maxBytes` 保形有界化）。
    pub async fn conversation_workflow_run_node_result_v4(
        &self,
        target: &AgentWorkspaceTarget,
        params: WorkflowRunNodeResultParams,
        policy: RuntimePolicy,
    ) -> Result<WorkflowRunNodeResult, ControlError> {
        let process = self.client_for(target, policy).await?;
        let value =
            serde_json::to_value(&params).expect("workflowRunNodeResult params 序列化不会失败");
        let response = process
            .client()
            .request(V4_CONVERSATION_WORKFLOW_RUN_NODE_RESULT_METHOD, Some(value))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!(
                "invalid v4/conversation/workflowRunNodeResult result: {error}"
            ))
        })
    }

    /// `v4/conversation/fileChanges`：某个行目标的文件变更聚合。
    pub async fn conversation_file_changes_v4(
        &self,
        target: &AgentWorkspaceTarget,
        params: FileChangesParams,
        policy: RuntimePolicy,
    ) -> Result<FileChangesResult, ControlError> {
        let process = self.client_for(target, policy).await?;
        let value = serde_json::to_value(&params).expect("fileChanges params 序列化不会失败");
        let response = process
            .client()
            .request(V4_CONVERSATION_FILE_CHANGES_METHOD, Some(value))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!("invalid v4/conversation/fileChanges result: {error}"))
        })
    }

    /// `v4/conversation/fileRewindPreview`：rewind 前的安全/不安全/忽略清单。
    pub async fn conversation_file_rewind_preview_v4(
        &self,
        target: &AgentWorkspaceTarget,
        params: FileRewindPreviewParams,
        policy: RuntimePolicy,
    ) -> Result<FileRewindPreviewResult, ControlError> {
        let process = self.client_for(target, policy).await?;
        let value =
            serde_json::to_value(&params).expect("fileRewindPreview params 序列化不会失败");
        let response = process
            .client()
            .request(V4_CONVERSATION_FILE_REWIND_PREVIEW_METHOD, Some(value))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!(
                "invalid v4/conversation/fileRewindPreview result: {error}"
            ))
        })
    }

    /// `v4/conversation/backgroundBashOutput`：按任务授权的有界输出查询。
    ///
    /// 与 TS 一致：existing-only（观察者绝不拉起进程）；方法不存在映射为 `unsupported`，
    /// 运行时不可用映射为 `unavailable`，其余错误上抛。
    pub async fn background_bash_output_v4(
        &self,
        target: &AgentWorkspaceTarget,
        params: BackgroundBashOutputParams,
    ) -> Result<BackgroundBashOutputResult, ControlError> {
        let work_id = params.work_id.clone();
        let process = match self.client_for(target, RuntimePolicy::ExistingOnly).await {
            Ok(process) => process,
            Err(ControlError::RuntimeUnavailable { .. }) => {
                return Ok(BackgroundBashOutputResult::Unavailable(
                    BackgroundBashUnavailable {
                        kind: BackgroundBashUnavailableKind::Unavailable,
                        work_id,
                        code: None,
                    },
                ));
            }
            Err(error) => return Err(error),
        };
        let value = serde_json::to_value(&params).expect("backgroundBashOutput params 序列化不会失败");
        let response = match process
            .client()
            .request(V4_BACKGROUND_BASH_OUTPUT_METHOD, Some(value))
            .await
        {
            Ok(response) => response,
            Err(ClientError::Server { code: -32601, .. }) => {
                return Ok(BackgroundBashOutputResult::Unavailable(
                    BackgroundBashUnavailable {
                        kind: BackgroundBashUnavailableKind::Unsupported,
                        work_id,
                        code: None,
                    },
                ));
            }
            Err(error) => return Err(ControlError::Client(error)),
        };
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!(
                "invalid v4/conversation/backgroundBashOutput result: {error}"
            ))
        })
    }

    /// `v4/conversation/usage`：会话级 token 用量（旧 `session/usage` 的 v4 落位）。
    pub async fn conversation_usage_v4(
        &self,
        target: &AgentWorkspaceTarget,
        params: ConversationUsageParams,
        policy: RuntimePolicy,
    ) -> Result<ConversationUsageResult, ControlError> {
        let process = self.client_for(target, policy).await?;
        let value = serde_json::to_value(&params).expect("conversation/usage params 序列化不会失败");
        let response = process
            .client()
            .request(V4_CONVERSATION_USAGE_METHOD, Some(value))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!(
                "invalid v4/conversation/usage result: {error}"
            ))
        })
    }

    /// `v4/attachment/begin`：开启（或幂等重现）一次附件上传事务。
    pub async fn attachment_begin_v4(
        &self,
        target: &AgentWorkspaceTarget,
        params: AttachmentBeginParams,
        policy: RuntimePolicy,
    ) -> Result<AttachmentBeginResult, ControlError> {
        params.validate().map_err(ControlError::InvalidParams)?;
        let process = self.client_for(target, policy).await?;
        let value = serde_json::to_value(&params).expect("attachment/begin params 序列化不会失败");
        let response = process
            .client()
            .request(V4_ATTACHMENT_BEGIN_METHOD, Some(value))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!("invalid v4/attachment/begin result: {error}"))
        })
    }

    /// `v4/attachment/chunk`：追加一块上传数据。
    pub async fn attachment_chunk_v4(
        &self,
        target: &AgentWorkspaceTarget,
        params: AttachmentChunkParams,
        policy: RuntimePolicy,
    ) -> Result<AttachmentChunkResult, ControlError> {
        let process = self.client_for(target, policy).await?;
        let value = serde_json::to_value(&params).expect("attachment/chunk params 序列化不会失败");
        let response = process
            .client()
            .request(V4_ATTACHMENT_CHUNK_METHOD, Some(value))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!("invalid v4/attachment/chunk result: {error}"))
        })
    }

    /// `v4/attachment/commit`：提交上传，返回可用的附件 `ref`。
    pub async fn attachment_commit_v4(
        &self,
        target: &AgentWorkspaceTarget,
        params: AttachmentTerminalParams,
        policy: RuntimePolicy,
    ) -> Result<AttachmentCommitResult, ControlError> {
        let process = self.client_for(target, policy).await?;
        let value = serde_json::to_value(&params).expect("attachment/commit params 序列化不会失败");
        let response = process
            .client()
            .request(V4_ATTACHMENT_COMMIT_METHOD, Some(value))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!("invalid v4/attachment/commit result: {error}"))
        })
    }

    /// `v4/attachment/abort`：放弃上传事务。
    pub async fn attachment_abort_v4(
        &self,
        target: &AgentWorkspaceTarget,
        params: AttachmentTerminalParams,
        policy: RuntimePolicy,
    ) -> Result<AttachmentAbortResult, ControlError> {
        let process = self.client_for(target, policy).await?;
        let value = serde_json::to_value(&params).expect("attachment/abort params 序列化不会失败");
        let response = process
            .client()
            .request(V4_ATTACHMENT_ABORT_METHOD, Some(value))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!("invalid v4/attachment/abort result: {error}"))
        })
    }

    /// `v4/attachment/read`：已发送附件的分段读（任意 MIME）。
    pub async fn attachment_read_v4(
        &self,
        target: &AgentWorkspaceTarget,
        params: AttachmentReadParams,
        policy: RuntimePolicy,
    ) -> Result<AttachmentReadResult, ControlError> {
        params.validate().map_err(ControlError::InvalidParams)?;
        let process = self.client_for(target, policy).await?;
        let value = serde_json::to_value(&params).expect("attachment/read params 序列化不会失败");
        let response = process
            .client()
            .request(V4_ATTACHMENT_READ_METHOD, Some(value))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!("invalid v4/attachment/read result: {error}"))
        })
    }

    /// `v4/attachment/previewSource`：Desktop local 已发送视频的本地播放源；远端回 chunked。
    pub async fn attachment_preview_source_v4(
        &self,
        target: &AgentWorkspaceTarget,
        params: AttachmentPreviewSourceParams,
        policy: RuntimePolicy,
    ) -> Result<AttachmentPreviewSourceResult, ControlError> {
        params.validate().map_err(ControlError::InvalidParams)?;
        let process = self.client_for(target, policy).await?;
        let value =
            serde_json::to_value(&params).expect("attachment/previewSource params 序列化不会失败");
        let response = process
            .client()
            .request(V4_ATTACHMENT_PREVIEW_SOURCE_METHOD, Some(value))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!(
                "invalid v4/attachment/previewSource result: {error}"
            ))
        })
    }

    /// `v4/conversation/attachmentRead`：userInput 附件的分段读（Share 选择阶段）。
    pub async fn conversation_attachment_read_v4(
        &self,
        target: &AgentWorkspaceTarget,
        params: ConversationAttachmentReadParams,
        policy: RuntimePolicy,
    ) -> Result<AttachmentReadResult, ControlError> {
        let process = self.client_for(target, policy).await?;
        let value = serde_json::to_value(&params)
            .expect("conversation/attachmentRead params 序列化不会失败");
        let response = process
            .client()
            .request(V4_CONVERSATION_ATTACHMENT_READ_METHOD, Some(value))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!(
                "invalid v4/conversation/attachmentRead result: {error}"
            ))
        })
    }

    /// `v4/conversation/attachmentStat`：metadata-only 探测（允许超过传输上限）。
    pub async fn conversation_attachment_stat_v4(
        &self,
        target: &AgentWorkspaceTarget,
        params: ConversationAttachmentStatParams,
        policy: RuntimePolicy,
    ) -> Result<ConversationAttachmentStatResult, ControlError> {
        let process = self.client_for(target, policy).await?;
        let value = serde_json::to_value(&params)
            .expect("conversation/attachmentStat params 序列化不会失败");
        let response = process
            .client()
            .request(V4_CONVERSATION_ATTACHMENT_STAT_METHOD, Some(value))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!(
                "invalid v4/conversation/attachmentStat result: {error}"
            ))
        })
    }

    /// `v4/connection/flow`：连接级流控状态（saturated / drained / closed）。
    pub async fn set_connection_flow_state_v4(
        &self,
        target: &AgentWorkspaceTarget,
        params: ConnectionFlowParams,
        policy: RuntimePolicy,
    ) -> Result<(), ControlError> {
        let process = self.client_for(target, policy).await?;
        let value = serde_json::to_value(&params).expect("connection/flow params 序列化不会失败");
        let response = process
            .client()
            .request(V4_CONNECTION_FLOW_METHOD, Some(value))
            .await?;
        serde_json::from_value::<ConnectionFlowResult>(response).map_err(|error| {
            ControlError::InvalidResult(format!("invalid v4/connection/flow result: {error}"))
        })?;
        Ok(())
    }

    /// `v4/conversation/resync`：按基线重同步一个订阅，返回新的 ACK。
    pub async fn resync_conversation_v4(
        &self,
        target: &AgentWorkspaceTarget,
        params: ConversationResyncParams,
        policy: RuntimePolicy,
    ) -> Result<ConversationResyncResult, ControlError> {
        let process = self.client_for(target, policy).await?;
        let value = serde_json::to_value(&params).expect("conversation/resync params 序列化不会失败");
        let response = process
            .client()
            .request(V4_CONVERSATION_RESYNC_METHOD, Some(value))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!(
                "invalid v4/conversation/resync result: {error}"
            ))
        })
    }

    /// `v4/conversation/unsubscribe`：释放一个订阅。
    pub async fn unsubscribe_conversation_v4(
        &self,
        target: &AgentWorkspaceTarget,
        params: ConversationUnsubscribeParams,
        policy: RuntimePolicy,
    ) -> Result<(), ControlError> {
        let process = self.client_for(target, policy).await?;
        let value =
            serde_json::to_value(&params).expect("conversation/unsubscribe params 序列化不会失败");
        let response = process
            .client()
            .request(V4_CONVERSATION_UNSUBSCRIBE_METHOD, Some(value))
            .await?;
        serde_json::from_value::<ConnectionFlowResult>(response).map_err(|error| {
            ControlError::InvalidResult(format!(
                "invalid v4/conversation/unsubscribe result: {error}"
            ))
        })?;
        Ok(())
    }

    /// `v4/usage/stats`：应用级用量聚合（事实源在 CLI usage store）。
    pub async fn usage_stats_v4(
        &self,
        target: &AgentWorkspaceTarget,
        params: UsageStatsParams,
        policy: RuntimePolicy,
    ) -> Result<AppUsageSnapshot, ControlError> {
        let process = self.client_for(target, policy).await?;
        let value = serde_json::to_value(&params).expect("usage/stats params 序列化不会失败");
        let response = process
            .client()
            .request(V4_USAGE_STATS_METHOD, Some(value))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!("invalid v4/usage/stats result: {error}"))
        })
    }
}
