//! `mode-agent` 控制面 facade（按方法分组迁移）。
//!
//! 第一组先搬 hooks 预信任所需的 `workspace/hooks/trustGrant`：构造 `workspace` ref、
//! 校验两个 sha256 摘要、把 Agent 的 `{accepted, reasonCode?}` 严格解析回来。
//!
//! 行为真源：`packages/services/src/mode-agent/modeAgentService.ts` 的
//! `grantWorkspaceHookTrust` 与 `buildWorkspaceRef`。

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::{AgentProcessError, AgentRuntimeRegistry, WorkspaceTarget};
use mode_protocol_client::ClientError;

pub const WORKSPACE_HOOK_TRUST_GRANT_METHOD: &str = "workspace/hooks/trustGrant";

/// TS `MODE_AGENT_RUNTIME_UNAVAILABLE_CODE`：existing-only 取不到运行时时的错误码。
pub const MODE_AGENT_RUNTIME_UNAVAILABLE_CODE: &str = "MODE_AGENT_RUNTIME_UNAVAILABLE";

/// 取 client 时是否允许拉起 Agent（TS `ModeAgentRuntimePolicy`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RuntimePolicy {
    #[default]
    StartIfNeeded,
    ExistingOnly,
}

#[derive(Debug, Clone)]
pub struct AgentWorkspaceTarget {
    pub workspace_path: String,
    pub workspace_identity: Option<String>,
    pub remote_session_id: Option<String>,
}

impl AgentWorkspaceTarget {
    pub fn new(workspace_path: impl Into<String>) -> Self {
        Self {
            workspace_path: workspace_path.into(),
            workspace_identity: None,
            remote_session_id: None,
        }
    }

    pub fn with_identity(mut self, identity: impl Into<String>) -> Self {
        self.workspace_identity = Some(identity.into());
        self
    }

    pub fn with_remote_session(mut self, remote_session_id: impl Into<String>) -> Self {
        self.remote_session_id = Some(remote_session_id.into());
        self
    }

    /// `workspaceIdentity?.trim() || workspacePath`
    pub fn workspace_key(&self) -> String {
        match self.workspace_identity.as_deref().map(str::trim) {
            Some(identity) if !identity.is_empty() => identity.to_string(),
            _ => self.workspace_path.clone(),
        }
    }

    pub(crate) fn registry_target(&self) -> WorkspaceTarget {
        WorkspaceTarget {
            workspace_path: self.workspace_path.clone(),
            workspace_identity: self.workspace_identity.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantTrustReasonCode {
    WorkspaceHooksBlockedByPolicy,
    WorkspaceHooksBundleChanged,
    WorkspaceHooksSnapshotMismatch,
    WorkspaceHooksPolicyRequiresPretrust,
    WorkspaceHooksTrustStoreCorrupt,
    WorkspaceHooksConfigUnreadable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantTrustResult {
    pub accepted: bool,
    pub reason_code: Option<GrantTrustReasonCode>,
}

#[derive(Debug)]
pub enum ControlError {
    InvalidDigest { field: &'static str },
    Spawn(AgentProcessError),
    Client(ClientError),
    InvalidResult(String),
    /// 调用方构造的协议参数不满足 schema 的组合约束（发出前拒绝）。
    InvalidParams(String),
    /// v4 命令被服务端否决（rejected/stale/failed），消息携带原始 ACK 摘要。
    CommandRejected(String),
    ProviderSync(String),
    /// existing-only 策略下没有活进程（对应 TS `MODE_AGENT_RUNTIME_UNAVAILABLE`）。
    RuntimeUnavailable { workspace_key: String },
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // 与 TS 抛 Error 的可见消息一致：Agent 的 message 原样上抛。
            Self::Client(ClientError::Server { message, .. }) => formatter.write_str(message),
            Self::Client(error) => write!(formatter, "{error}"),
            Self::Spawn(error) => write!(formatter, "{error}"),
            Self::InvalidDigest { field } => write!(
                formatter,
                "{field} must be a 64-character lowercase sha256 digest"
            ),
            Self::InvalidResult(message) => formatter.write_str(message),
            Self::InvalidParams(message) => formatter.write_str(message),
            Self::CommandRejected(message) => formatter.write_str(message),
            Self::ProviderSync(message) => formatter.write_str(message),
            // 消息与 TS `createRuntimeUnavailableError` 一致。
            Self::RuntimeUnavailable { .. } => formatter.write_str("Mode Agent runtime is not running."),
        }
    }
}

impl std::error::Error for ControlError {}

impl From<AgentProcessError> for ControlError {
    fn from(error: AgentProcessError) -> Self {
        Self::Spawn(error)
    }
}

impl From<ClientError> for ControlError {
    fn from(error: ClientError) -> Self {
        Self::Client(error)
    }
}

pub struct AgentControlClient {
    registry: Arc<AgentRuntimeRegistry>,
    provider_sync: crate::ProviderConfigSync,
    /// v4 命令的稳定提交端标识（对应 TS host services 的进程稳定 clientId）。
    v4_client_id: String,
}

impl AgentControlClient {
    pub fn new(registry: Arc<AgentRuntimeRegistry>) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        Self {
            registry,
            provider_sync: crate::ProviderConfigSync::new(),
            v4_client_id: format!("host-rust-{}-{now}", std::process::id()),
        }
    }

    pub(crate) fn v4_client_id(&self) -> &str {
        &self.v4_client_id
    }

    /// `getReadOnlyClient(params, runtimePolicy)` 的 Rust 形态。
    ///
    /// - `StartIfNeeded`：复用或拉起；
    /// - `ExistingOnly`：只复用现有活进程，绝不 spawn。
    pub(crate) async fn client_for(
        &self,
        target: &AgentWorkspaceTarget,
        policy: RuntimePolicy,
    ) -> Result<Arc<crate::AgentProcess>, ControlError> {
        match policy {
            RuntimePolicy::StartIfNeeded => Ok(self
                .registry
                .get_or_spawn(&target.registry_target())
                .await?),
            RuntimePolicy::ExistingOnly => self
                .registry
                .existing_process(&target.registry_target())
                .ok_or_else(|| ControlError::RuntimeUnavailable {
                    workspace_key: target.workspace_key(),
                }),
        }
    }

    /// Account Provider Config 交付；未注入源时与 TS 的「无源」分支一致，直接返回。
    pub async fn sync_account_provider_config(
        &self,
        target: &AgentWorkspaceTarget,
        source: Option<&dyn crate::ProviderSource>,
    ) -> Result<Option<crate::ProviderSyncOutcome>, ControlError> {
        let Some(source) = source else {
            return Ok(None);
        };
        let process = self.registry.get_or_spawn(&target.registry_target()).await?;
        self.provider_sync
            .sync(&target.workspace_key(), process.client(), source)
            .await
            .map(Some)
    }

    /// 存储启动：拉起（如需）并等待 `startup/storageState` 到 `ready`。
    pub async fn prepare_storage(&self, target: &AgentWorkspaceTarget) -> Result<(), String> {
        self.registry
            .prepare_storage(&target.registry_target())
            .await
    }

    /// 现有进程的存储启动快照（不 spawn）。
    pub fn storage_startup_state(
        &self,
        target: &AgentWorkspaceTarget,
    ) -> Option<crate::StorageStartupSnapshot> {
        self.registry
            .storage_startup_state(&target.registry_target())
    }

    /// 现有 runtime 的身份（existing-only，不 spawn）。
    ///
    /// 对应 TS `getWorkspaceRuntimeIdentity` 的 existing-only 语义；调用方在 `None` 时
    /// 自行映射错误（TS 抛 `Mode agent runtime identity is unavailable.`）。
    pub fn runtime_identity(&self, target: &AgentWorkspaceTarget) -> Option<crate::RuntimeIdentity> {
        self.registry.existing_identity(&target.registry_target())
    }

    /// 关闭并移除该 workspace 的 Agent（对应 `disposeWorkspace`）。
    pub async fn dispose_workspace(&self, target: &AgentWorkspaceTarget, grace: std::time::Duration) {
        self.registry
            .invalidate(&target.workspace_key(), grace)
            .await;
    }

    /// 关闭全部 Agent（对应 `disposeAll`）。
    pub async fn dispose_all(&self, grace: std::time::Duration) {
        self.registry.shutdown_all(grace).await;
    }

    /// 订阅现有进程的存储状态；没有活进程时返回 `None`。
    pub fn subscribe_storage_startup(
        &self,
        target: &AgentWorkspaceTarget,
    ) -> Option<tokio::sync::watch::Receiver<Option<crate::StorageStartupState>>> {
        self.registry
            .subscribe_storage_startup(&target.registry_target())
    }

    /// `grantWorkspaceHookTrust`：没有 task/session 的 workspace 级预信任。
    /// 真正授权由 Agent 重新发现 canonical snapshot 后完成。
    pub async fn grant_workspace_hook_trust(
        &self,
        target: &AgentWorkspaceTarget,
        bundle_digest: &str,
        hook_declaration_digest: &str,
    ) -> Result<GrantTrustResult, ControlError> {
        validate_digest("bundleDigest", bundle_digest)?;
        validate_digest("hookDeclarationDigest", hook_declaration_digest)?;
        let process = self.registry.get_or_spawn(&target.registry_target()).await?;
        let response = process
            .client()
            .request(
                WORKSPACE_HOOK_TRUST_GRANT_METHOD,
                Some(build_grant_params(
                    target,
                    bundle_digest,
                    hook_declaration_digest,
                )),
            )
            .await?;
        parse_grant_result(&response)
    }
}

/// `buildWorkspaceRef`：`undefined` 字段省略，`workspaceKey` 恒在。
pub fn build_workspace_ref(target: &AgentWorkspaceTarget) -> Value {
    let mut object = Map::new();
    object.insert(
        "workspacePath".into(),
        Value::String(target.workspace_path.clone()),
    );
    if let Some(identity) = &target.workspace_identity {
        object.insert("workspaceIdentity".into(), Value::String(identity.clone()));
    }
    if let Some(remote_session_id) = &target.remote_session_id {
        object.insert(
            "remoteSessionId".into(),
            Value::String(remote_session_id.clone()),
        );
    }
    object.insert(
        "workspaceKey".into(),
        Value::String(target.workspace_key()),
    );
    Value::Object(object)
}

pub fn build_grant_params(
    target: &AgentWorkspaceTarget,
    bundle_digest: &str,
    hook_declaration_digest: &str,
) -> Value {
    json!({
        "workspace": build_workspace_ref(target),
        "bundleDigest": bundle_digest,
        "hookDeclarationDigest": hook_declaration_digest,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GrantResultWire {
    accepted: bool,
    reason_code: Option<GrantTrustReasonCode>,
}

fn parse_grant_result(value: &Value) -> Result<GrantTrustResult, ControlError> {
    let wire: GrantResultWire = serde_json::from_value(value.clone()).map_err(|error| {
        ControlError::InvalidResult(format!("invalid workspaceHookTrustGrant result: {error}"))
    })?;
    Ok(GrantTrustResult {
        accepted: wire.accepted,
        reason_code: wire.reason_code,
    })
}

fn validate_digest(field: &'static str, value: &str) -> Result<(), ControlError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Ok(());
    }
    Err(ControlError::InvalidDigest { field })
}

