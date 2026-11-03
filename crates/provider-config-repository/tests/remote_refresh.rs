use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use provider_config::{ModelConfigRules, ProviderConfigMap, ProviderTemplateMap};
use provider_config_repository::{
    acquire_refresh_lease, finish_refresh_lease, read_refresh_control,
    refresh_mode_builtin_release, AcquireOptions, AcquireOutcome, ApplyOutcome, FinishOptions,
    LeaseFinish, RefreshOptions, RefreshResult, SkipReason,
};
use provider_config_source::ModeBuiltinRelease;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pcrr-test-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn release(revision: i64) -> ModeBuiltinRelease {
    ModeBuiltinRelease {
        revision,
        providers: ProviderConfigMap::empty(),
        provider_templates: ProviderTemplateMap::empty(),
        models: ModelConfigRules::empty(),
    }
}

fn options<'a>(path: &'a Path, force: bool, now_ms: i64) -> RefreshOptions<'a> {
    RefreshOptions {
        control_file_path: path,
        endpoint_key: "https://example.com",
        lease_id: "lease-main",
        force,
        now_ms,
        lease_duration_ms: 30_000,
        success_interval_ms: 3_600_000,
        failure_base_delay_ms: 60_000,
        failure_max_delay_ms: 3_600_000,
    }
}

fn set_control(path: &Path, endpoint: &str, lease: &str, now: i64) {
    let outcome = acquire_refresh_lease(
        path,
        &AcquireOptions {
            endpoint_key: endpoint,
            lease_id: lease,
            force: true,
            now_ms: now,
            lease_duration_ms: 30_000,
        },
    )
    .expect("acquire");
    assert!(matches!(outcome, AcquireOutcome::Acquired(_)));
}

fn finish_success(path: &Path, endpoint: &str, lease: &str, now: i64) {
    finish_refresh_lease(
        path,
        &FinishOptions {
            endpoint_key: endpoint,
            lease_id: lease,
            finish: LeaseFinish::Success,
            now_ms: now,
            success_interval_ms: 3_600_000,
            failure_base_delay_ms: 60_000,
            failure_max_delay_ms: 3_600_000,
        },
    )
    .expect("finish");
}

#[test]
fn active_lease_skips_without_fetch() {
    let dir = temp_dir();
    let path = dir.join("control.json");
    set_control(&path, "https://example.com", "other-lease", 1_000);
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_slot = Arc::clone(&calls);
    let outcome = refresh_mode_builtin_release(
        &options(&path, false, 2_000),
        move |_| {
            calls_slot.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        },
        || Ok("https://example.com".to_string()),
        |_| Ok(ApplyOutcome::Updated),
    )
    .expect("refresh");
    assert_eq!(outcome.result, RefreshResult::Skipped);
    assert_eq!(outcome.reason, Some(SkipReason::LeaseHeld));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn not_due_skips_unless_forced() {
    let dir = temp_dir();
    let path = dir.join("control.json");
    set_control(&path, "https://example.com", "lease-0", 1_000);
    finish_success(&path, "https://example.com", "lease-0", 1_000);

    let outcome = refresh_mode_builtin_release(
        &options(&path, false, 2_000),
        |_| Ok(None),
        || Ok("https://example.com".to_string()),
        |_| Ok(ApplyOutcome::Updated),
    )
    .expect("refresh");
    assert_eq!(outcome.result, RefreshResult::Skipped);
    assert_eq!(outcome.reason, Some(SkipReason::NotDue));

    let forced = refresh_mode_builtin_release(
        &options(&path, true, 2_000),
        |_| Ok(Some(release(5))),
        || Ok("https://example.com".to_string()),
        |_| Ok(ApplyOutcome::Updated),
    )
    .expect("forced refresh");
    assert_eq!(forced.result, RefreshResult::Updated);
    assert_eq!(forced.revision, Some(5));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn apply_outcomes_map_to_refresh_results() {
    for (applied, expected) in [
        (ApplyOutcome::Updated, RefreshResult::Updated),
        (ApplyOutcome::Unchanged, RefreshResult::Unchanged),
        (ApplyOutcome::Stale, RefreshResult::Stale),
    ] {
        let dir = temp_dir();
        let path = dir.join("control.json");
        let outcome = refresh_mode_builtin_release(
            &options(&path, false, 1_000),
            |_| Ok(Some(release(7))),
            || Ok("https://example.com".to_string()),
            move |_| Ok(applied),
        )
        .expect("refresh");
        assert_eq!(outcome.result, expected);
        assert_eq!(outcome.revision, Some(7));
        let control = read_refresh_control(&path).unwrap();
        assert_eq!(control.failure_count, 0);
        assert_eq!(control.lease_id, None);
        assert_eq!(control.next_eligible_at, 1_000 + 3_600_000);
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[test]
fn missing_release_is_reported_and_lease_finishes_success() {
    let dir = temp_dir();
    let path = dir.join("control.json");
    let outcome = refresh_mode_builtin_release(
        &options(&path, false, 1_000),
        |_| Ok(None),
        || Ok("https://example.com".to_string()),
        |_| Ok(ApplyOutcome::Updated),
    )
    .expect("refresh");
    assert_eq!(outcome.result, RefreshResult::Missing);
    assert_eq!(outcome.revision, None);
    let control = read_refresh_control(&path).unwrap();
    assert_eq!(control.failure_count, 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn endpoint_change_skips_before_apply() {
    let dir = temp_dir();
    let path = dir.join("control.json");
    let applied = Arc::new(AtomicUsize::new(0));
    let applied_slot = Arc::clone(&applied);
    let outcome = refresh_mode_builtin_release(
        &options(&path, false, 1_000),
        |_| Ok(Some(release(1))),
        || Ok("https://other.example.com".to_string()),
        move |_| {
            applied_slot.fetch_add(1, Ordering::SeqCst);
            Ok(ApplyOutcome::Updated)
        },
    )
    .expect("refresh");
    assert_eq!(outcome.result, RefreshResult::Skipped);
    assert_eq!(outcome.reason, Some(SkipReason::EndpointChanged));
    assert_eq!(applied.load(Ordering::SeqCst), 0);
    let control = read_refresh_control(&path).unwrap();
    assert_eq!(control.failure_count, 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn fetch_error_finishes_lease_as_failure() {
    let dir = temp_dir();
    let path = dir.join("control.json");
    let error = refresh_mode_builtin_release(
        &options(&path, false, 1_000),
        |_| Err("network down".to_string()),
        || Ok("https://example.com".to_string()),
        |_| Ok(ApplyOutcome::Updated),
    )
    .unwrap_err();
    assert_eq!(error, "network down");
    let control = read_refresh_control(&path).unwrap();
    assert_eq!(control.failure_count, 1);
    assert_eq!(control.lease_id, None);
    assert_eq!(control.next_eligible_at, 1_000 + 60_000);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn apply_error_finishes_lease_as_failure() {
    let dir = temp_dir();
    let path = dir.join("control.json");
    let error = refresh_mode_builtin_release(
        &options(&path, false, 1_000),
        |_| Ok(Some(release(1))),
        || Ok("https://example.com".to_string()),
        |_| Err("apply failed".to_string()),
    )
    .unwrap_err();
    assert_eq!(error, "apply failed");
    let control = read_refresh_control(&path).unwrap();
    assert_eq!(control.failure_count, 1);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn endpoint_resolution_error_finishes_lease_as_failure() {
    let dir = temp_dir();
    let path = dir.join("control.json");
    let error = refresh_mode_builtin_release(
        &options(&path, false, 1_000),
        |_| Ok(Some(release(1))),
        || Err("endpoint resolve failed".to_string()),
        |_| Ok(ApplyOutcome::Updated),
    )
    .unwrap_err();
    assert_eq!(error, "endpoint resolve failed");
    let control = read_refresh_control(&path).unwrap();
    assert_eq!(control.failure_count, 1);
    std::fs::remove_dir_all(&dir).ok();
}
