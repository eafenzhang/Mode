//! M2 Task 4 / DR-T4-7 (c)：host 层 `CommandInbox` 的准入顺序。
//!
//! 规格：`docs/specs/tauri-agent-runtime.md` §3.4.1。这些用例**不断言等待时长**——
//! 队列没有超时，顺序完全由 `admit` / `complete` / `cancel_queued` 的调用顺序决定。
//! 跨层对拍（会话面的 `-32010` 拒绝 vs host 队列）等 host 面可观察后再补。

use agent_core::command_inbox::{Admission, CommandInbox, Completion};

#[test]
fn first_admission_starts_and_the_rest_queue_in_order() {
    let mut inbox = CommandInbox::new();
    assert_eq!(inbox.admit("session", "a"), Admission::Started);
    assert_eq!(
        inbox.admit("session", "b"),
        Admission::Queued { position: 1 }
    );
    assert_eq!(
        inbox.admit("session", "c"),
        Admission::Queued { position: 2 }
    );
    assert_eq!(inbox.running("session"), Some("a"));
    assert_eq!(inbox.pending("session"), vec!["b", "c"]);
}

#[test]
fn completion_promotes_in_fifo_order() {
    let mut inbox = CommandInbox::new();
    inbox.admit("session", "a");
    inbox.admit("session", "b");
    inbox.admit("session", "c");

    assert_eq!(
        inbox.complete("session", "a"),
        Completion::Promoted("b".to_string())
    );
    assert_eq!(inbox.running("session"), Some("b"));
    assert_eq!(
        inbox.complete("session", "b"),
        Completion::Promoted("c".to_string())
    );
    assert_eq!(inbox.complete("session", "c"), Completion::Idle);
    assert_eq!(inbox.running("session"), None);
    assert!(inbox.pending("session").is_empty());
}

#[test]
fn stale_completion_does_not_touch_the_queue() {
    let mut inbox = CommandInbox::new();
    inbox.admit("session", "a");
    inbox.admit("session", "b");

    assert_eq!(inbox.complete("session", "b"), Completion::Stale);
    assert_eq!(inbox.running("session"), Some("a"));
    assert_eq!(inbox.pending("session"), vec!["b"]);
    assert_eq!(inbox.complete("unknown", "a"), Completion::Stale);
}

#[test]
fn keys_are_independent() {
    let mut inbox = CommandInbox::new();
    assert_eq!(inbox.admit("session-1", "a"), Admission::Started);
    assert_eq!(inbox.admit("session-2", "x"), Admission::Started);
    assert_eq!(inbox.running("session-1"), Some("a"));
    assert_eq!(inbox.running("session-2"), Some("x"));

    inbox.complete("session-1", "a");
    assert_eq!(inbox.running("session-2"), Some("x"));
}

#[test]
fn only_queued_commands_can_be_cancelled_in_place() {
    let mut inbox = CommandInbox::new();
    inbox.admit("session", "a");
    inbox.admit("session", "b");
    inbox.admit("session", "c");

    assert!(inbox.cancel_queued("session", "b"));
    assert_eq!(inbox.pending("session"), vec!["c"]);
    // running 不能被 cancel_queued 移除：取消 running 必须以终态 complete 收口。
    assert!(!inbox.cancel_queued("session", "a"));
    assert_eq!(inbox.running("session"), Some("a"));
    // 重复取消/未知 id 是无操作。
    assert!(!inbox.cancel_queued("session", "b"));
    assert!(!inbox.cancel_queued("unknown", "a"));
}
