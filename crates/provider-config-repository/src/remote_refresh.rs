//! Mode Built-in Remote Release 刷新编排：租约 → fetch → 端点复核 → apply → finish。
//!
//! 真源：`packages/provider-node/src/mode-builtin-remote-synchronizer.ts` 的
//! `#refresh`。网络与 apply 由调用方注入；取消/AbortSignal 与 in-flight 去重留在宿主。

use std::path::Path;

use provider_config_source::ModeBuiltinRelease;

use crate::remote_sync::{
    acquire_refresh_lease, finish_refresh_lease, AcquireOptions, AcquireOutcome, FinishOptions,
    LeaseFinish,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshResult {
    Updated,
    Unchanged,
    Stale,
    Missing,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    LeaseHeld,
    NotDue,
    EndpointChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    Updated,
    Unchanged,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshOutcome {
    pub result: RefreshResult,
    pub reason: Option<SkipReason>,
    pub revision: Option<i64>,
}

pub struct RefreshOptions<'a> {
    pub control_file_path: &'a Path,
    pub endpoint_key: &'a str,
    pub lease_id: &'a str,
    pub force: bool,
    pub now_ms: i64,
    pub lease_duration_ms: i64,
    pub success_interval_ms: i64,
    pub failure_base_delay_ms: i64,
    pub failure_max_delay_ms: i64,
}

/// 执行一次刷新；租约被占/未到期时返回 `Skipped`，网络或 apply 失败时结束租约并返回错误。
pub fn refresh_mode_builtin_release<F, R, A>(
    options: &RefreshOptions<'_>,
    fetch_release: F,
    resolve_endpoint_key: R,
    apply_release: A,
) -> Result<RefreshOutcome, String>
where
    F: FnOnce(&str) -> Result<Option<ModeBuiltinRelease>, String>,
    R: FnOnce() -> Result<String, String>,
    A: FnOnce(ModeBuiltinRelease) -> Result<ApplyOutcome, String>,
{
    let acquire = acquire_refresh_lease(
        options.control_file_path,
        &AcquireOptions {
            endpoint_key: options.endpoint_key,
            lease_id: options.lease_id,
            force: options.force,
            now_ms: options.now_ms,
            lease_duration_ms: options.lease_duration_ms,
        },
    )?;
    match acquire {
        AcquireOutcome::LeaseHeld => {
            return Ok(skipped(SkipReason::LeaseHeld));
        }
        AcquireOutcome::NotDue => {
            return Ok(skipped(SkipReason::NotDue));
        }
        AcquireOutcome::Acquired(_) => {}
    }

    let release = match fetch_release(options.endpoint_key) {
        Ok(release) => release,
        Err(error) => {
            finish(options, LeaseFinish::Failure)?;
            return Err(error);
        }
    };

    let current_endpoint = match resolve_endpoint_key() {
        Ok(endpoint) => endpoint,
        Err(error) => {
            finish(options, LeaseFinish::Failure)?;
            return Err(error);
        }
    };
    if current_endpoint.trim() != options.endpoint_key {
        finish(options, LeaseFinish::Success)?;
        return Ok(skipped(SkipReason::EndpointChanged));
    }

    let Some(release) = release else {
        finish(options, LeaseFinish::Success)?;
        return Ok(RefreshOutcome {
            result: RefreshResult::Missing,
            reason: None,
            revision: None,
        });
    };
    let revision = release.revision;
    let applied = match apply_release(release) {
        Ok(outcome) => outcome,
        Err(error) => {
            finish(options, LeaseFinish::Failure)?;
            return Err(error);
        }
    };
    finish(options, LeaseFinish::Success)?;
    Ok(RefreshOutcome {
        result: match applied {
            ApplyOutcome::Updated => RefreshResult::Updated,
            ApplyOutcome::Unchanged => RefreshResult::Unchanged,
            ApplyOutcome::Stale => RefreshResult::Stale,
        },
        reason: None,
        revision: Some(revision),
    })
}

fn finish(options: &RefreshOptions<'_>, finish: LeaseFinish) -> Result<(), String> {
    finish_refresh_lease(
        options.control_file_path,
        &FinishOptions {
            endpoint_key: options.endpoint_key,
            lease_id: options.lease_id,
            finish,
            now_ms: options.now_ms,
            success_interval_ms: options.success_interval_ms,
            failure_base_delay_ms: options.failure_base_delay_ms,
            failure_max_delay_ms: options.failure_max_delay_ms,
        },
    )?;
    Ok(())
}

fn skipped(reason: SkipReason) -> RefreshOutcome {
    RefreshOutcome {
        result: RefreshResult::Skipped,
        reason: Some(reason),
        revision: None,
    }
}
