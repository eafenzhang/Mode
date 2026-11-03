use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use provider_config_repository::{
    acquire_refresh_lease, finish_refresh_lease, read_refresh_control, write_refresh_control,
    AcquireOptions, AcquireOutcome, FinishOptions, LeaseFinish, RefreshControl,
};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pcrs-test-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn acquire(path: &Path, endpoint: &str, lease: &str, force: bool, now: i64) -> AcquireOutcome {
    acquire_refresh_lease(
        path,
        &AcquireOptions {
            endpoint_key: endpoint,
            lease_id: lease,
            force,
            now_ms: now,
            lease_duration_ms: 30_000,
        },
    )
    .expect("acquire")
}

fn finish(
    path: &Path,
    endpoint: &str,
    lease: &str,
    outcome: LeaseFinish,
    now: i64,
) -> Option<RefreshControl> {
    finish_refresh_lease(
        path,
        &FinishOptions {
            endpoint_key: endpoint,
            lease_id: lease,
            finish: outcome,
            now_ms: now,
            success_interval_ms: 3_600_000,
            failure_base_delay_ms: 60_000,
            failure_max_delay_ms: 3_600_000,
        },
    )
    .expect("finish")
}

#[test]
fn first_acquire_writes_lease() {
    let dir = temp_dir();
    let path = dir.join("control.json");
    let outcome = acquire(&path, "endpoint-a", "lease-1", false, 1_000);
    let AcquireOutcome::Acquired(control) = outcome else {
        panic!("应取得租约");
    };
    assert_eq!(control.endpoint_key, "endpoint-a");
    assert_eq!(control.lease_id.as_deref(), Some("lease-1"));
    assert_eq!(control.lease_until, 31_000);
    assert_eq!(control.next_eligible_at, 0);
    assert_eq!(control.failure_count, 0);
    assert_eq!(read_refresh_control(&path).unwrap(), control);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn active_lease_blocks_other_refreshes() {
    let dir = temp_dir();
    let path = dir.join("control.json");
    acquire(&path, "endpoint-a", "lease-1", false, 1_000);
    assert_eq!(
        acquire(&path, "endpoint-a", "lease-2", false, 2_000),
        AcquireOutcome::LeaseHeld
    );
    assert_eq!(
        read_refresh_control(&path).unwrap().lease_id.as_deref(),
        Some("lease-1")
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn success_sets_next_window_and_force_bypasses_it() {
    let dir = temp_dir();
    let path = dir.join("control.json");
    acquire(&path, "endpoint-a", "lease-1", false, 1_000);
    let finished =
        finish(&path, "endpoint-a", "lease-1", LeaseFinish::Success, 2_000).expect("finished");
    assert_eq!(finished.lease_until, 0);
    assert_eq!(finished.next_eligible_at, 3_602_000);
    assert_eq!(finished.failure_count, 0);
    assert_eq!(
        acquire(&path, "endpoint-a", "lease-2", false, 3_000),
        AcquireOutcome::NotDue
    );
    assert!(matches!(
        acquire(&path, "endpoint-a", "lease-2", true, 3_000),
        AcquireOutcome::Acquired(_)
    ));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn endpoint_change_resets_failure_and_window() {
    let dir = temp_dir();
    let path = dir.join("control.json");
    let control = RefreshControl {
        schema_version: 1,
        endpoint_key: "endpoint-a".to_string(),
        lease_id: None,
        lease_until: 0,
        next_eligible_at: 9_999_999,
        failure_count: 3,
    };
    write_refresh_control(&path, &control).unwrap();
    let outcome = acquire(&path, "endpoint-b", "lease-1", false, 10);
    let AcquireOutcome::Acquired(control) = outcome else {
        panic!("端点变化后应可刷新");
    };
    assert_eq!(control.next_eligible_at, 0);
    assert_eq!(control.failure_count, 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn failure_backoff_doubles_and_caps() {
    let dir = temp_dir();
    let path = dir.join("control.json");
    acquire(&path, "endpoint-a", "lease-1", false, 0);
    let first = finish(&path, "endpoint-a", "lease-1", LeaseFinish::Failure, 1_000).unwrap();
    assert_eq!(first.failure_count, 1);
    assert_eq!(first.next_eligible_at, 61_000);

    acquire(&path, "endpoint-a", "lease-2", true, 61_000);
    let second = finish(&path, "endpoint-a", "lease-2", LeaseFinish::Failure, 62_000).unwrap();
    assert_eq!(second.failure_count, 2);
    assert_eq!(second.next_eligible_at, 182_000);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn cancelled_keeps_failure_count_without_delay() {
    let dir = temp_dir();
    let path = dir.join("control.json");
    let control = RefreshControl {
        schema_version: 1,
        endpoint_key: "endpoint-a".to_string(),
        lease_id: Some("lease-1".to_string()),
        lease_until: 5_000,
        next_eligible_at: 0,
        failure_count: 2,
    };
    write_refresh_control(&path, &control).unwrap();
    let finished = finish(
        &path,
        "endpoint-a",
        "lease-1",
        LeaseFinish::Cancelled,
        1_000,
    )
    .expect("finished");
    assert_eq!(finished.failure_count, 2);
    assert_eq!(finished.next_eligible_at, 1_000);
    assert_eq!(finished.lease_until, 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn mismatched_lease_does_not_write() {
    let dir = temp_dir();
    let path = dir.join("control.json");
    acquire(&path, "endpoint-a", "lease-1", false, 0);
    let before = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        finish(&path, "endpoint-a", "lease-2", LeaseFinish::Success, 1_000),
        None
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn corrupt_control_file_is_replaced_by_empty_control() {
    let dir = temp_dir();
    let path = dir.join("control.json");
    std::fs::write(&path, "{broken").unwrap();
    assert_eq!(
        read_refresh_control(&path).unwrap(),
        RefreshControl::empty()
    );
    assert!(matches!(
        acquire(&path, "endpoint-a", "lease-1", false, 0),
        AcquireOutcome::Acquired(_)
    ));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn missing_parent_directories_are_created() {
    let dir = temp_dir();
    let path = dir.join("nested").join("control.json");
    assert!(read_refresh_control(&path).unwrap() == RefreshControl::empty());
    assert!(matches!(
        acquire(&path, "endpoint-a", "lease-1", false, 0),
        AcquireOutcome::Acquired(_)
    ));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn empty_endpoint_is_rejected() {
    let dir = temp_dir();
    let path = dir.join("control.json");
    let error = acquire_refresh_lease(
        &path,
        &AcquireOptions {
            endpoint_key: "   ",
            lease_id: "lease-1",
            force: false,
            now_ms: 0,
            lease_duration_ms: 30_000,
        },
    )
    .unwrap_err();
    assert!(error.contains("Endpoint"));
    std::fs::remove_dir_all(&dir).ok();
}
