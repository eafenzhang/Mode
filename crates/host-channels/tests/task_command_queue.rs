//! 任务运行时命令队列行为契约（LT-16.2b）。
//!
//! owner 门控三态、enqueue/promote/cancel 状态转换、drain 单发消费
//! （成功移除 / 失败标 failed / active-run 与 running 门控跳过）。

use std::sync::Arc;

use agent_protocol::session::SessionSendParams;
use agent_process::AgentWorkspaceTarget;
use host_channels::task_command_queue::{
    QueueCommandParams, RuntimeCommandStatus, TaskCommandQueue,
};

fn params(task_id: &str, command_id: &str) -> QueueCommandParams {
    QueueCommandParams {
        command_id: command_id.to_string(),
        task_id: task_id.to_string(),
        trace_id: Some(format!("trace-{command_id}")),
        query_id: None,
        workspace_path: "D:/work/app".to_string(),
        workspace_identity: None,
        content: format!("prompt for {command_id}"),
        automation_id: None,
        attachments: None,
    }
}

fn queue(recorded: Arc<std::sync::Mutex<Vec<String>>>) -> Arc<TaskCommandQueue> {
    let executor: host_channels::task_command_queue::SendExecutor =
        Arc::new(move |_target: AgentWorkspaceTarget, _params: SessionSendParams| {
            let recorded = Arc::clone(&recorded);
            Box::pin(async move {
                recorded.lock().expect("record").push("sent".to_string());
                Ok(())
            })
        });
    Arc::new(TaskCommandQueue::new(executor, Arc::new(|| 1_000)))
}

fn key_of(task_id: &str) -> String {
    format!("D:/work/app\u{0000}{task_id}")
}

#[tokio::test]
async fn enqueue_and_drain_sends_and_removes() {
    let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
    let queue = queue(Arc::clone(&recorded));
    let result = queue
        .enqueue(&params("task-1", "cmd-1"), None, None, None)
        .expect("enqueue");
    assert!(result.accepted);
    assert_eq!(result.command.status, RuntimeCommandStatus::Accepted);

    // drain 单次：发送成功 → 命令移除。
    queue.drain_once(&key_of("task-1"), "D:/work/app", None).await;
    assert_eq!(recorded.lock().expect("record").len(), 1);
    assert!(queue.snapshot_commands(&key_of("task-1")).is_empty());
}

#[tokio::test]
async fn drain_failure_marks_failed_and_keeps_command() {
    let queue = Arc::new(TaskCommandQueue::new(
        Arc::new(|_target, _params| {
            Box::pin(async move { Err("agent unreachable".to_string()) })
        }),
        Arc::new(|| 1_000),
    ));
    queue
        .enqueue(&params("task-1", "cmd-1"), None, None, None)
        .expect("enqueue");
    queue.drain_once(&key_of("task-1"), "D:/work/app", None).await;
    let commands = queue.snapshot_commands(&key_of("task-1"));
    assert_eq!(commands.len(), 1, "失败命令留在队列（UI 可见）");
    assert_eq!(commands[0].status, RuntimeCommandStatus::Failed);
    assert_eq!(commands[0].error.as_deref(), Some("agent unreachable"));
}

#[tokio::test]
async fn active_run_and_running_gate_skip_drain() {
    let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
    let queue = queue(Arc::clone(&recorded));
    queue
        .enqueue(&params("task-1", "cmd-1"), None, None, None)
        .expect("enqueue");

    // active run 进行中：drain 跳过（prompt 还没收口）。
    queue.set_active_run_for_test(&key_of("task-1"), "run-1");
    queue.drain_once(&key_of("task-1"), "D:/work/app", None).await;
    assert_eq!(recorded.lock().expect("record").len(), 0, "active run 门控");
    queue.clear_active_run_for_test(&key_of("task-1"));

    // drain 成功移除后再 drain：空队列无事发生。
    queue.drain_once(&key_of("task-1"), "D:/work/app", None).await;
    assert_eq!(recorded.lock().expect("record").len(), 1);
}

#[tokio::test]
async fn owner_gate_rejects_stale_and_missing_active_run() {
    let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
    let queue = queue(Arc::clone(&recorded));
    queue.set_active_run_for_test(&key_of("task-1"), "run-2");

    // stale：旧 run 不能改当前队列。
    let error = queue
        .enqueue(&params("task-1", "cmd-stale"), Some("run-1"), None, None)
        .expect_err("stale");
    match error {
        rpc_host::ChannelError::Error { message, .. } => {
            assert!(message.contains("STALE_TASK_OWNER_COMMAND"));
        }
        other => panic!("期望 Error 变体，实际 {other:?}"),
    }

    // 无 active run：不允许写入（避免误发旧输入）。
    queue.clear_active_run_for_test(&key_of("task-1"));
    let error = queue
        .enqueue(&params("task-1", "cmd-orphan"), Some("run-1"), None, None)
        .expect_err("no active owner");
    match error {
        rpc_host::ChannelError::Error { message, .. } => {
            assert!(message.contains("NO_ACTIVE_TASK_OWNER"));
        }
        other => panic!("期望 Error 变体，实际 {other:?}"),
    }

    // ownerRunId 缺省（桌面 continuous）跳过门控。
    let result = queue
        .enqueue(&params("task-1", "cmd-local"), None, None, None)
        .expect("local");
    assert!(result.accepted);
    queue.drain_once(&key_of("task-1"), "D:/work/app", None).await;
    assert_eq!(recorded.lock().expect("record").len(), 1);
}

#[tokio::test]
async fn promote_moves_accepted_to_front_and_cancel_removes() {
    let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
    let queue = queue(Arc::clone(&recorded));
    queue
        .enqueue(&params("task-1", "cmd-1"), None, None, None)
        .expect("enqueue 1");
    queue
        .enqueue(&params("task-1", "cmd-2"), None, None, None)
        .expect("enqueue 2");

    // promote cmd-1（原来在队首之后？enqueue 顺序 cmd-1, cmd-2——promote 后 cmd-1 仍队首）。
    // 先 promote cmd-2 到队首，再 drain：应先消费 cmd-2。
    let promoted = queue
        .promote(&params("task-1", "cmd-2"), "cmd-2", None)
        .expect("promote");
    assert_eq!(promoted.command.command_id, "cmd-2");

    // cancel cmd-1：移除。
    let canceled = queue
        .cancel(&params("task-1", "cmd-1"), "cmd-1", None)
        .expect("cancel");
    assert!(canceled.canceled);
    assert_eq!(canceled.reason, "removed");

    // drain：消费队首 cmd-2（cmd-1 已移除）。
    queue.drain_once(&key_of("task-1"), "D:/work/app", None).await;
    assert_eq!(recorded.lock().expect("record").len(), 1);
    let commands = queue.snapshot_commands(&key_of("task-1"));
    assert!(commands.is_empty(), "cmd-2 发送成功后移除");

    // cancel 不存在的命令：容忍。
    let canceled = queue
        .cancel(&params("task-1", "cmd-absent"), "cmd-absent", None)
        .expect("cancel absent");
    assert_eq!(canceled.reason, "not_found");
}
