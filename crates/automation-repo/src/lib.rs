//! 定时任务仓库（`AutomationRepo`）的 Rust 实现。
//!
//! 行为真源（clean-room：只读本仓库 TS）：
//!
//! - `packages/services/src/session/automationRepo.ts`
//! - `packages/shared/src/automation-types.ts`
//! - `packages/shared/src/bots.ts`（`modeAutomationBotDeliveryTargetSchema`）
//! - `packages/shared/src/model-selection.ts`
//! - `packages/shared/src/mode-task-types-core.ts`（`modeTaskModeSchema`）
//!
//! 数据库 schema 与冻结迁移由 `tasks-db` 提供。规范见
//! `docs/specs/automation-repo.md`。

mod error;
mod repo;

pub use error::{AutomationRepoError, Result};
pub use repo::{
    compute_retry_at, Automation, AutomationBotDeliveryTarget, AutomationCreateParams,
    AutomationCreateOptions, AutomationRepo, AutomationRestartOptions, AutomationRun,
    AutomationScope, AutomationUpdateOptions, AutomationUpdateParams, ClaimedManualAutomationRun,
    DispatchFailureKind, EnsureRunClaimedParams, MarkDispatchFailedOptions, MarkDispatchedOptions,
    MarkManualRunDispatchedParams, MarkRunDispatchParams, RecordSkippedRunParams, RunNowOptions,
    SkipAndRescheduleParams, UpsertRunClaimedParams, AUTOMATION_CREATE_LIMIT, AUTOMATION_CREATE_LIMIT_ERROR_CODE,
    CLAIM_STALE_MS, DISPATCH_MAX_ATTEMPTS, DISPATCH_RETRY_BASE_MS, DISPATCH_RETRY_CAP_MS,
};
pub use model_selection::{parse_model_selection, ModelSelection, SelectionOptions};
