//! 被控会话状态机契约（`remote-desktop` 会话面）。
//!
//! 真源 `packages/desktop/src/host/remoteDesktopServer.ts`。验四件事：
//! 准入顺序、单会话互斥、两个 TTL、被控态只在真的变化时通知。

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use host_channels::remote_desktop_session::{
    ConsentResult, ControlledSession, ControlledSessionMachine, CONSENT_SAFETY_TIMEOUT_MS,
    PENDING_SOCKET_TTL_MS,
};
use rpc_wire::Value;

/// 可推进的假时钟。
fn clock() -> (Arc<AtomicI64>, Arc<dyn Fn() -> i64 + Send + Sync>) {
    let now = Arc::new(AtomicI64::new(1_000_000));
    let reader = Arc::clone(&now);
    (now, Arc::new(move || reader.load(Ordering::SeqCst)))
}

fn machine() -> (Arc<ControlledSessionMachine>, Arc<AtomicI64>) {
    let (now, reader) = clock();
    (Arc::new(ControlledSessionMachine::new(reader)), now)
}

#[test]
fn kill_switch_and_disabled_server_reject_before_anything_else() {
    let (machine, _now) = machine();

    // 总开关关：disabled。
    machine.set_admission_facts(false, false);
    assert_eq!(machine.request_session("c1"), Err("disabled"));

    // kill-switch 命中：即使总开关开也 disabled（只能关不能开）。
    machine.set_admission_facts(true, true);
    assert_eq!(machine.request_session("c1"), Err("disabled"));

    // 两者都放开：进入同意流。
    machine.set_admission_facts(false, true);
    assert!(machine.request_session("c1").is_ok());
}

#[test]
fn second_request_is_busy_and_never_opens_a_second_consent() {
    let (machine, _now) = machine();
    machine.set_admission_facts(false, true);

    machine.request_session("c1").expect("首个请求");
    // 单主控单被控：占用中直接 busy，不弹第二个同意窗。
    assert_eq!(machine.request_session("c2"), Err("busy"));

    let session = machine.session();
    assert_eq!(
        session.client_id(),
        Some("c1"),
        "第二个请求不得顶掉第一个：{session:?}"
    );
    assert_eq!(session.as_str(), "pending_consent");
}

#[test]
fn consent_allow_moves_to_pending_socket_and_deny_returns_to_idle() {
    let (machine, now) = machine();
    machine.set_admission_facts(false, true);

    let deadline = machine.request_session("c1").expect("请求");
    assert_eq!(
        deadline,
        now.load(Ordering::SeqCst) + CONSENT_SAFETY_TIMEOUT_MS
    );

    machine
        .respond_consent("c1", ConsentResult::Allow)
        .expect("allow");
    assert_eq!(machine.session().as_str(), "pending_socket");

    // 第二个会话在 pending_socket 期间同样 busy。
    assert_eq!(machine.request_session("c2"), Err("busy"));

    machine.stop("test");
    machine.request_session("c1").expect("重新请求");
    machine
        .respond_consent("c1", ConsentResult::Deny("consent_denied"))
        .expect("deny");
    // 拒绝后回到空闲：可以再次请求。
    assert_eq!(machine.session(), ControlledSession::Idle);
    assert!(machine.request_session("c1").is_ok());
}

#[test]
fn consent_response_for_the_wrong_client_is_rejected() {
    let (machine, _now) = machine();
    machine.set_admission_facts(false, true);
    machine.request_session("c1").expect("请求");

    // 迟到的/错主的回帖不能结算别人的同意——否则 A 的同意窗会被 B 关掉。
    let error = machine
        .respond_consent("c2", ConsentResult::Allow)
        .expect_err("错主必须失败");
    assert!(error.contains("没有待处理的同意请求"), "{error}");
    assert_eq!(machine.session().client_id(), Some("c1"));
}

#[test]
fn consent_safety_timeout_releases_the_occupancy() {
    let (machine, now) = machine();
    machine.set_admission_facts(false, true);
    machine.request_session("c1").expect("请求");

    // 未到期不释放。
    now.fetch_add(CONSENT_SAFETY_TIMEOUT_MS - 1, Ordering::SeqCst);
    assert_eq!(machine.tick(), None);
    assert!(machine.is_controlled());

    // 到期释放：否则一次「同意窗永远不回帖」会把被控端永久卡在 busy。
    now.fetch_add(2, Ordering::SeqCst);
    let expired = machine.tick().expect("必须过期");
    assert_eq!(expired.as_str(), "pending_consent");
    assert_eq!(machine.session(), ControlledSession::Idle);
    assert!(!machine.is_controlled());
}

#[test]
fn pending_socket_ttl_releases_busy_for_a_client_that_never_connects() {
    let (machine, now) = machine();
    machine.set_admission_facts(false, true);
    machine.request_session("c1").expect("请求");
    machine
        .respond_consent("c1", ConsentResult::Allow)
        .expect("allow");

    now.fetch_add(PENDING_SOCKET_TTL_MS - 1, Ordering::SeqCst);
    assert_eq!(machine.tick(), None);

    now.fetch_add(2, Ordering::SeqCst);
    let expired = machine.tick().expect("必须过期");
    assert_eq!(expired.as_str(), "pending_socket");
    // 一次「同意后从不连」不能把被控端永久卡在 busy。
    assert!(machine.request_session("c2").is_ok());
}

#[test]
fn active_session_occupies_and_only_its_own_socket_can_attach() {
    let (machine, _now) = machine();
    machine.set_admission_facts(false, true);
    machine.request_session("c1").expect("请求");
    machine
        .respond_consent("c1", ConsentResult::Allow)
        .expect("allow");

    // 别人的接线不能推进状态。
    let error = machine.attach_socket("c2").expect_err("错主接线必须失败");
    assert!(error.contains("没有等待接线的会话"), "{error}");

    machine.attach_socket("c1").expect("接线");
    assert_eq!(machine.session().as_str(), "active");
    assert!(machine.is_controlled());
    assert_eq!(machine.request_session("c3"), Err("busy"));
}

#[test]
fn stop_terminates_whichever_state_and_reports_it() {
    let (machine, _now) = machine();
    machine.set_admission_facts(false, true);

    // 空闲时 stop 无事可做。
    assert_eq!(machine.stop("idle"), None);

    machine.request_session("c1").expect("请求");
    let stopped = machine
        .stop("server disabled")
        .expect("必须报告被终止的会话");
    assert_eq!(stopped.as_str(), "pending_consent");
    assert_eq!(machine.session(), ControlledSession::Idle);

    // 关开关的终止路径要能让下一次请求重新进入。
    assert!(machine.request_session("c2").is_ok());
}

#[test]
fn controlled_state_is_notified_only_on_real_changes() {
    let (machine, now) = machine();
    machine.set_admission_facts(false, true);

    let events: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    let subscription = machine.subscribe(Arc::new(move |value: Value| {
        if let Value::Json(text) = value {
            let parsed: serde_json::Value = serde_json::from_str(&text).expect("json");
            sink.lock()
                .expect("events")
                .push(parsed["controlled"].as_bool().unwrap_or(false));
        }
    }));

    // 幂等事实：首个订阅者立即收到当前值（false）。
    assert_eq!(*events.lock().expect("events"), vec![false]);

    machine.request_session("c1").expect("请求");
    machine
        .respond_consent("c1", ConsentResult::Allow)
        .expect("allow");
    // pending_consent → pending_socket 仍是「被控中」：不该再发一条。
    machine.attach_socket("c1").expect("接线");
    // active 也是被控中：同样不发。
    machine.stop("bye");

    assert_eq!(
        *events.lock().expect("events"),
        vec![false, true, false],
        "只在 false→true 与 true→false 各发一次"
    );

    // 退订后不再收到。
    subscription();
    machine.request_session("c2").expect("请求");
    assert_eq!(events.lock().expect("events").len(), 3, "退订后不得再通知");

    // 未到期不会自己消失（确认上一行的「没收到」不是因为过期）。
    now.fetch_add(1, Ordering::SeqCst);
    assert_eq!(machine.tick(), None);
}

#[test]
fn dispose_stops_the_session_and_rejects_new_requests() {
    let (machine, _now) = machine();
    machine.set_admission_facts(false, true);
    machine.request_session("c1").expect("请求");

    let stopped = machine.dispose().expect("dispose 必须终止会话");
    assert_eq!(stopped.as_str(), "pending_consent");
    assert_eq!(
        machine.request_session("c1"),
        Err("disabled"),
        "dispose 之后一律 disabled（与 kill-switch 同优先级的死状态）"
    );
}
