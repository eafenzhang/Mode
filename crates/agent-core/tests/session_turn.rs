//! `turn.started.turnNumber` 的起点（Task 5 Step 0 观察）。
//!
//! 观察（`docs/specs/tauri-agent-runtime.md` §3.5）：第一个 turn 的 `turnNumber` 是 **0**，
//! 不是 1；后续 turn 逐次 +1。

use agent_core::session::Session;
use agent_core::SessionEvent;
use agent_protocol::session_event::TurnStartedPayload;

fn turn_number(event: &SessionEvent) -> u64 {
    event
        .payload_as::<TurnStartedPayload>()
        .expect("turn.started 载荷能解码")
        .expect("载荷类型匹配")
        .turn_number
}

#[test]
fn turn_numbers_start_at_zero_and_increment() {
    let mut session = Session::new(
        "sess_turn_number",
        std::env::temp_dir().to_string_lossy().to_string(),
    );
    assert_eq!(turn_number(&session.begin_turn("first")), 0);
    assert_eq!(turn_number(&session.begin_turn("second")), 1);
}
