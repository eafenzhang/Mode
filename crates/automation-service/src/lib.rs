//! 定时任务管理服务（`AutomationService` 的 Rust 侧）。
//!
//! 真源：`packages/services/src/session/automationService.ts`。本切片覆盖不涉及
//! create/update 调度推断的 8 个方法：list / listAll / delete / setEnabled / restart /
//! runNow / listRuns / deleteRun。create/update 的 carrier、锚点与生命周期重算在下一片。
//!
//! `restart` 需要按当前时间重算 nextRunAt，时区偏移由调用方注入（与 `automation-schedule`
//! 的 `offset_secs` 口径一致）。

mod types;

use automation_repo::{
    AutomationRepo, AutomationRestartOptions, AutomationScope, RunNowOptions,
};
use automation_schedule::compute_automation_next_run_at;

pub use types::{ModeAutomation, ModeAutomationRun, ModeAutomationRunNow};

pub struct AutomationServiceOptions {
    pub offset_secs: i32,
}

pub struct AutomationService {
    repo: AutomationRepo,
    offset_secs: i32,
}

impl AutomationService {
    pub fn new(repo: AutomationRepo, options: AutomationServiceOptions) -> Self {
        Self {
            repo,
            offset_secs: options.offset_secs,
        }
    }

    pub fn list(&self, scope: &AutomationScope) -> Result<Vec<ModeAutomation>, String> {
        self.repo
            .list(scope)
            .map(|items| items.iter().map(ModeAutomation::from).collect())
            .map_err(|error| error.to_string())
    }

    pub fn list_all(&self) -> Result<Vec<ModeAutomation>, String> {
        self.list(&AutomationScope::default())
    }

    pub fn delete(&self, automation_id: &str, scope: &AutomationScope) -> Result<bool, String> {
        self.repo
            .delete(automation_id, scope_key(scope).as_deref())
            .map_err(|error| error.to_string())
    }

    pub fn set_enabled(
        &self,
        automation_id: &str,
        enabled: bool,
        scope: &AutomationScope,
    ) -> Result<(), String> {
        self.repo
            .set_enabled(automation_id, enabled, scope_key(scope).as_deref())
            .map_err(|error| error.to_string())
    }

    /// 失败任务手动重跑：仅 `failed` 可恢复；按 cron/规则重算下次时间。
    pub fn restart(
        &self,
        automation_id: &str,
        scope: &AutomationScope,
        now: i64,
    ) -> Result<(), String> {
        let workspace_key = scope_key(scope);
        let Some(existing) = self
            .repo
            .get(automation_id, workspace_key.as_deref())
            .map_err(|error| error.to_string())?
        else {
            return Ok(());
        };
        if existing.lifecycle_status != "failed" {
            return Ok(());
        }
        let schedule_rule = types::parse_schedule_rule(existing.schedule_rule.as_ref());
        let next_run_at = compute_automation_next_run_at(
            &existing.cron_expr,
            schedule_rule.as_ref(),
            now,
            self.offset_secs,
        );
        self.repo
            .restart(
                automation_id,
                &AutomationRestartOptions { next_run_at },
                workspace_key.as_deref(),
            )
            .map_err(|error| error.to_string())
    }

    /// 立即运行：写 manual run 并占用 single-flight claim，不修改 cron 计划。
    pub fn run_now(
        &self,
        automation_id: &str,
        scope: &AutomationScope,
        now: i64,
    ) -> Result<Option<ModeAutomationRunNow>, String> {
        let workspace_key = scope_key(scope);
        let claimed = self
            .repo
            .run_now(
                automation_id,
                &RunNowOptions { now },
                workspace_key.as_deref(),
            )
            .map_err(|error| error.to_string())?;
        Ok(claimed.map(|claimed| ModeAutomationRunNow {
            automation: ModeAutomation::from(&claimed.automation),
            run: ModeAutomationRun::from(&claimed.run),
        }))
    }

    pub fn list_runs(
        &self,
        automation_id: &str,
        scope: &AutomationScope,
    ) -> Result<Vec<ModeAutomationRun>, String> {
        self.repo
            .list_runs(automation_id, scope_key(scope).as_deref())
            .map(|items| items.iter().map(ModeAutomationRun::from).collect())
            .map_err(|error| error.to_string())
    }

    pub fn delete_run(&self, run_id: &str, scope: &AutomationScope) -> Result<(), String> {
        self.repo
            .delete_run(run_id, scope_key(scope).as_deref())
            .map_err(|error| error.to_string())
    }

    pub fn repo(&self) -> &AutomationRepo {
        &self.repo
    }
}

/// `resolveScopeKey`：没有 workspacePath 时返回 None；否则 identity 优先。
fn scope_key(scope: &AutomationScope) -> Option<String> {
    let workspace_path = scope.workspace_path.as_deref()?.trim();
    if workspace_path.is_empty() {
        return None;
    }
    match scope.workspace_identity.as_deref().map(str::trim) {
        Some(identity) if !identity.is_empty() => Some(identity.to_string()),
        _ => Some(workspace_path.to_string()),
    }
}
