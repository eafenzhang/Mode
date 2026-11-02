//! 闲时任务服务端客户端的注入面与同步类型。
//!
//! 真源：`packages/services/src/session/offPeakServerClient.ts`。本 crate 只依赖
//! 这些稳定结果类型；HTTP/凭证/重试由真实实现负责。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OffPeakTakeTicketResult {
    pub ticket_id: String,
    pub state: String,
    pub position: Option<i64>,
    pub next_poll_after_ms: Option<i64>,
    pub registered_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OffPeakTicketStatusEntry {
    pub ticket_id: String,
    pub state: String,
    pub position: Option<i64>,
    pub active_deadline: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OffPeakBatchStatusResult {
    pub next_poll_after_ms: Option<i64>,
    pub tickets: Vec<OffPeakTicketStatusEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OffPeakTakeNumberAvailability {
    pub can_take_number: bool,
    pub next_take_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OffPeakClientError {
    Server {
        http_status: u16,
        biz_code: Option<i64>,
    },
    InvalidResponse,
    Network,
}

pub trait OffPeakServerClient: Send + Sync {
    fn get_take_number_availability(
        &self,
    ) -> Result<OffPeakTakeNumberAvailability, OffPeakClientError>;
    fn take_ticket(&self, task_id: &str) -> Result<OffPeakTakeTicketResult, OffPeakClientError>;
    fn batch_status(
        &self,
        ticket_ids: &[String],
    ) -> Result<OffPeakBatchStatusResult, OffPeakClientError>;
    fn settle(&self, ticket_id: &str) -> Result<(), OffPeakClientError>;
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OffPeakTaskCreateParams {
    pub title: String,
    pub prompt: String,
    pub permission_mode: String,
    pub model_selection: model_selection::ModelSelection,
    pub workspace_path: String,
    #[serde(default)]
    pub workspace_identity: Option<String>,
    #[serde(default)]
    pub bound_session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OffPeakTaskCreateSuccess {
    pub ok: bool,
    pub task: off_peak_repo::OffPeakTask,
    pub ticket_initial_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_position: Option<i64>,
    pub provider_name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OffPeakTaskCreateFailure {
    pub ok: bool,
    pub failure_stage: String,
    pub error_category: String,
    pub error_code: String,
    pub provider_name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum OffPeakTaskCreateResult {
    Success(Box<OffPeakTaskCreateSuccess>),
    Failure(OffPeakTaskCreateFailure),
}
