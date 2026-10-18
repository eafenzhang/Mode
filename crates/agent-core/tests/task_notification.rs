//! 后台完成通知（M2 Task 5C）：XML 形状与「空闲时开新 turn」。
//!
//! 期望文案来自 `docs/specs/tauri-agent-runtime.md` §3.4 的实测；
//! 忙时丢弃是 oracle 的行为（spec §3.5），由调用方只在空闲时调用本方法来保证。

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agent_core::provider::{ScriptStep, ScriptedProvider};
use agent_core::session::Session;
use agent_core::SessionEventType;
use agent_protocol::session_event::TurnStartedPayload;
use agent_tools::TaskStatus;

fn bash_program() -> PathBuf {
    #[cfg(windows)]
    {
        for candidate in [
            r"C:\Program Files\Git\bin\bash.exe",
            r"C:\Program Files\Git\usr\bin\bash.exe",
            r"C:\Program Files (x86)\Git\bin\bash.exe",
        ] {
            let path = PathBuf::from(candidate);
            if path.exists() {
                return path;
            }
        }
    }
    #[cfg(not(windows))]
    {
        let path = PathBuf::from("/bin/bash");
        if path.exists() {
            return path;
        }
    }
    PathBuf::from("bash")
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mode-task-notify-{tag}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

fn spawn_script(tag: &str, script: &str) -> (std::process::Child, PathBuf) {
    let dir = temp_dir(tag);
    let path = dir.join("out.log");
    let file = fs::File::create(&path).expect("建输出文件");
    let child = Command::new(bash_program())
        .arg("-lc")
        .arg(format!("exec 2>&1\n{script}"))
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn 测试进程");
    (child, path)
}

#[test]
fn completed_task_notification_matches_the_observed_xml() {
    let session = Session::new("sess_notify", temp_dir("completed").to_string_lossy());
    let registry = session.tasks().clone();
    let (child, path) = spawn_script("completed", "printf 'bg-done\\n'");
    let registration = registry.register(
        child,
        "toolu_bg_0",
        "printf 'bg-done\\n'",
        Some("Quick background".to_string()),
        path,
    );
    registry
        .snapshot(&registration.id, true, Duration::from_secs(5))
        .expect("任务应完成");

    let notifications = session.take_task_notifications();
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].status, TaskStatus::Completed);
    let xml = notifications[0].to_xml();
    assert!(xml.starts_with("<task-notification>"));
    assert!(xml.contains(&format!("<task-id>{}</task-id>", registration.id)));
    assert!(xml.contains("<tool-use-id>toolu_bg_0</tool-use-id>"));
    assert!(xml.contains("<status>completed</status>"));
    assert!(xml.contains(
        "<summary>Background command \"Quick background\" completed (exit code 0)</summary>"
    ));
}

#[test]
fn pending_notifications_run_as_a_new_turn_when_idle() {
    let mut session = Session::new("sess_notify", temp_dir("run").to_string_lossy());
    let registry = session.tasks().clone();
    let (child, path) = spawn_script("run", "printf 'bg-done\\n'");
    let registration = registry.register(
        child,
        "toolu_bg_3",
        "printf 'bg-done\\n'",
        Some("Quick background".to_string()),
        path,
    );
    registry
        .snapshot(&registration.id, true, Duration::from_secs(5))
        .expect("任务应完成");

    let provider = ScriptedProvider::new(vec![ScriptStep::Text("handled".to_string())]);
    let mut sink = Vec::new();
    let count = session
        .run_pending_notifications(&provider, &mut sink)
        .expect("通知 turn 应成功");
    assert_eq!(count, 1);
    assert_eq!(sink[0].event_type, SessionEventType::TurnStarted);
    let started = sink[0]
        .payload_as::<TurnStartedPayload>()
        .expect("能解码")
        .expect("类型匹配");
    assert!(started.input.starts_with("<task-notification>"));
    assert!(started.input.contains("<status>completed</status>"));
}

#[test]
fn failed_task_notification_uses_the_observed_summary() {
    let session = Session::new("sess_notify", temp_dir("failed").to_string_lossy());
    let registry = session.tasks().clone();
    let (child, path) = spawn_script("failed", "printf 'bad\\n'; exit 7");
    let registration = registry.register(
        child,
        "toolu_bg_1",
        "printf 'bad\\n'; exit 7",
        Some("Failing background".to_string()),
        path,
    );
    registry
        .snapshot(&registration.id, true, Duration::from_secs(5))
        .expect("任务应结束");

    let notifications = session.take_task_notifications();
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].status, TaskStatus::Failed);
    let xml = notifications[0].to_xml();
    assert!(xml.contains("<status>failed</status>"));
    assert!(xml.contains(
        "<summary>Background command \"Failing background\" failed with exit code 7</summary>"
    ));
}

#[test]
fn stopped_task_notification_is_killed_and_says_stopped() {
    let session = Session::new("sess_notify", temp_dir("stopped").to_string_lossy());
    let registry = session.tasks().clone();
    let (child, path) = spawn_script("stopped", "sleep 30");
    let registration = registry.register(
        child,
        "toolu_bg_2",
        "sleep 30",
        Some("Long background".to_string()),
        path,
    );

    registry.stop(&registration.id).expect("应能停止");
    let notifications = session.take_task_notifications();
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].status, TaskStatus::Cancelled);
    let xml = notifications[0].to_xml();
    assert!(xml.contains("<status>killed</status>"));
    assert!(xml.contains("<summary>Background command \"Long background\" was stopped</summary>"));
}
