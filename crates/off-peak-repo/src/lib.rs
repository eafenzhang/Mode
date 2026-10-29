//! 闲时任务仓库（`OffPeakTaskRepo`）的 Rust 实现。
//!
//! 行为真源（clean-room：只读本仓库 TS）：
//!
//! - `packages/services/src/session/offPeakTaskRepo.ts`
//! - `packages/shared/src/off-peak-types.ts`
//! - `packages/shared/src/model-selection.ts`（`modelSelectionSchema`）
//! - `packages/shared/src/task-realtime-core.ts`（`resolveWorkspaceKey`）
//!
//! 数据库 schema 与冻结迁移由 `tasks-db` 提供。规范见
//! `docs/specs/off-peak-repo.md`。

mod error;
mod repo;
mod selection;

pub use error::{OffPeakRepoError, Result};
pub use repo::{
    CreateOptions, CreateParams, EditableFields, EditableModelSelection, InvalidateOptions,
    MarkRunningOptions, MarkTerminalOptions, ModelSelectionIssue, OffPeakTask, OffPeakTaskRepo,
    ReleaseClaimOptions,
    SchedulingPatch, TaskScope, OFF_PEAK_CLAIM_STALE_MS,
};
pub use selection::{parse_model_selection, ModelSelection, SelectionOptions};
