use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use cua_raw_input::{RawInputSocketResolver, RawInputSocketResolverOptions};

struct Harness {
    resolver: RawInputSocketResolver,
    live: Arc<Mutex<Option<String>>>,
    stable: Arc<Mutex<Option<String>>>,
    ensure_result: Arc<Mutex<Option<String>>>,
    enabled: Arc<AtomicBool>,
    ensure_calls: Arc<AtomicUsize>,
    now: Arc<AtomicU64>,
}

fn harness() -> Harness {
    let live = Arc::new(Mutex::new(None));
    let stable = Arc::new(Mutex::new(None));
    let ensure_result = Arc::new(Mutex::new(None));
    let enabled = Arc::new(AtomicBool::new(false));
    let ensure_calls = Arc::new(AtomicUsize::new(0));
    let now = Arc::new(AtomicU64::new(1_000));
    let ensure_wired = Arc::new(AtomicBool::new(true));

    let live_slot = Arc::clone(&live);
    let stable_slot = Arc::clone(&stable);
    let enabled_slot = Arc::clone(&enabled);
    let ensure_slot = Arc::clone(&ensure_result);
    let ensure_counter = Arc::clone(&ensure_calls);
    let now_slot = Arc::clone(&now);
    let wired = Arc::clone(&ensure_wired);

    let resolver = RawInputSocketResolver::new(RawInputSocketResolverOptions {
        resolve_live_socket_path: Arc::new(move || {
            live_slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }),
        probe_stable_socket_path: Arc::new(move || {
            stable_slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }),
        is_remote_desktop_server_enabled: Arc::new(move || enabled_slot.load(Ordering::SeqCst)),
        ensure_remote_desktop_helper: {
            let ensure_slot = Arc::clone(&ensure_slot);
            let ensure_counter = Arc::clone(&ensure_counter);
            let wired = Arc::clone(&wired);
            if wired.load(Ordering::SeqCst) {
                Some(Arc::new(move || {
                    ensure_counter.fetch_add(1, Ordering::SeqCst);
                    ensure_slot
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .clone()
                }))
            } else {
                None
            }
        },
        ensure_failure_backoff_ms: 5_000,
        now_ms: Arc::new(move || now_slot.load(Ordering::SeqCst)),
    });

    Harness {
        resolver,
        live,
        stable,
        ensure_result,
        enabled,
        ensure_calls,
        now,
    }
}

#[test]
fn live_socket_wins_without_probe_or_ensure() {
    let mut h = harness();
    *h.live.lock().unwrap() = Some("live-socket".to_string());
    *h.stable.lock().unwrap() = Some("stable-socket".to_string());
    *h.ensure_result.lock().unwrap() = Some("ensured-socket".to_string());
    h.enabled.store(true, Ordering::SeqCst);

    assert_eq!(h.resolver.resolve().as_deref(), Some("live-socket"));
    assert_eq!(h.ensure_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn stable_probe_wins_without_ensure() {
    let mut h = harness();
    *h.stable.lock().unwrap() = Some("stable-socket".to_string());
    *h.ensure_result.lock().unwrap() = Some("ensured-socket".to_string());
    h.enabled.store(true, Ordering::SeqCst);

    assert_eq!(h.resolver.resolve().as_deref(), Some("stable-socket"));
    assert_eq!(h.ensure_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn disabled_switch_never_ensures() {
    let mut h = harness();
    *h.ensure_result.lock().unwrap() = Some("ensured-socket".to_string());
    assert_eq!(h.resolver.resolve(), None);
    assert_eq!(h.ensure_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn unwired_ensure_is_probe_only() {
    let mut resolver = RawInputSocketResolver::new(RawInputSocketResolverOptions {
        is_remote_desktop_server_enabled: Arc::new(|| true),
        ..Default::default()
    });
    assert_eq!(resolver.resolve(), None);
}

#[test]
fn ensure_success_returns_socket() {
    let mut h = harness();
    h.enabled.store(true, Ordering::SeqCst);
    *h.ensure_result.lock().unwrap() = Some("ensured-socket".to_string());
    assert_eq!(h.resolver.resolve().as_deref(), Some("ensured-socket"));
    assert_eq!(h.ensure_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn failed_ensure_backs_off_until_clock_advances() {
    let mut h = harness();
    h.enabled.store(true, Ordering::SeqCst);

    assert_eq!(h.resolver.resolve(), None);
    assert_eq!(h.ensure_calls.load(Ordering::SeqCst), 1);

    h.now.store(4_000, Ordering::SeqCst);
    assert_eq!(h.resolver.resolve(), None, "退避窗口内不再创建");
    assert_eq!(h.ensure_calls.load(Ordering::SeqCst), 1);

    h.now.store(6_000, Ordering::SeqCst);
    *h.ensure_result.lock().unwrap() = Some("ensured-socket".to_string());
    assert_eq!(h.resolver.resolve().as_deref(), Some("ensured-socket"));
    assert_eq!(h.ensure_calls.load(Ordering::SeqCst), 2);
}

#[test]
fn zero_backoff_retries_every_call() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_slot = Arc::clone(&calls);
    let mut resolver = RawInputSocketResolver::new(RawInputSocketResolverOptions {
        is_remote_desktop_server_enabled: Arc::new(|| true),
        ensure_remote_desktop_helper: Some(Arc::new(move || {
            calls_slot.fetch_add(1, Ordering::SeqCst);
            None
        })),
        ensure_failure_backoff_ms: 0,
        ..Default::default()
    });
    assert_eq!(resolver.resolve(), None);
    assert_eq!(resolver.resolve(), None);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
