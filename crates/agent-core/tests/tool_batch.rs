//! 工具批次编排（LT-1 C1 最终评审 I1）：整批先 `scheduled`，随后串行执行且
//! `started`/`result` 紧贴执行边界。
//!
//! 观察出处：`docs/specs/agent-core-parity.md` §5.2 与 `tauri-agent-runtime.md`
//! §C1.5-6；多调用批次的 oracle 行为夹具尚未录制（登记为回放缺口），但
//! 「调度先于执行」「started 先于完成」由本次用例直接钉住。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use agent_core::session::{Session, ToolCallRequest};
use agent_core::{EventSink, SessionEvent, ToolUpdatedPayload};
use serde_json::json;

struct TempWorkspace(PathBuf);

impl TempWorkspace {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "agent-core-tool-batch-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("建临时工作区");
        Self(path)
    }

    fn cwd(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempWorkspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn tool_kinds(events: &[SessionEvent]) -> Vec<&'static str> {
    events
        .iter()
        .filter_map(|event| event.payload_as::<ToolUpdatedPayload>().expect("能解码"))
        .map(|payload| match payload {
            ToolUpdatedPayload::Scheduled(_) => "scheduled",
            ToolUpdatedPayload::Started(_) => "started",
            ToolUpdatedPayload::Progress(_) => "progress",
            ToolUpdatedPayload::Result(_) => "result",
            ToolUpdatedPayload::Error(_) => "error",
            ToolUpdatedPayload::Batch(_) => "batch",
        })
        .collect()
}

#[test]
fn multi_call_batch_schedules_all_calls_before_first_started() {
    let workspace = TempWorkspace::new("schedule-all");
    std::fs::write(workspace.cwd().join("a.txt"), "a\n").expect("种子 a");
    std::fs::write(workspace.cwd().join("b.txt"), "b\n").expect("种子 b");

    let mut session = Session::new("sess_batch", workspace.cwd().to_string_lossy().to_string());
    let _ = session.begin_turn("读两个文件");
    let calls = vec![
        ToolCallRequest {
            tool_call_id: "call_a".to_string(),
            name: "Read".to_string(),
            input: json!({ "file_path": format!("{}/a.txt", workspace.cwd().display()) }),
        },
        ToolCallRequest {
            tool_call_id: "call_b".to_string(),
            name: "Read".to_string(),
            input: json!({ "file_path": format!("{}/b.txt", workspace.cwd().display()) }),
        },
    ];
    let events = session.run_tool_batch(&calls);

    let kinds = tool_kinds(&events);
    // 串行编排：两个 scheduled 先到齐，然后 call_a 的 started/result，再 call_b 的
    // started/result，最后 batch。
    assert_eq!(
        kinds,
        vec![
            "scheduled",
            "scheduled",
            "started",
            "result",
            "started",
            "result",
            "batch"
        ],
        "多调用批次必须整批先调度、再串行执行"
    );

    let first_started = events
        .iter()
        .position(|event| {
            matches!(
                event.payload_as::<ToolUpdatedPayload>().expect("能解码"),
                Some(ToolUpdatedPayload::Started(_))
            )
        })
        .expect("有 started");
    for event in &events[..first_started] {
        assert!(
            matches!(
                event.payload_as::<ToolUpdatedPayload>().expect("能解码"),
                Some(ToolUpdatedPayload::Scheduled(_))
            ),
            "第一个 started 之前只能是 scheduled 事件"
        );
    }
}

/// 慢工具：`started` 必须在该调用**开始执行之前**投递，`result` 在其完成后投递。
#[test]
fn slow_tool_started_is_emitted_before_execution_completes() {
    #[derive(Default)]
    struct TimingSink {
        started_at: Option<Instant>,
        result_at: Option<Instant>,
    }

    impl EventSink for TimingSink {
        fn emit(&mut self, event: SessionEvent) {
            match event.payload_as::<ToolUpdatedPayload>().expect("能解码") {
                Some(ToolUpdatedPayload::Started(_)) if self.started_at.is_none() => {
                    self.started_at = Some(Instant::now());
                }
                Some(ToolUpdatedPayload::Result(_)) if self.result_at.is_none() => {
                    self.result_at = Some(Instant::now());
                }
                _ => {}
            }
        }
    }

    let workspace = TempWorkspace::new("slow-started");
    let mut session = Session::new("sess_slow", workspace.cwd().to_string_lossy().to_string());
    let _ = session.begin_turn("等一秒");
    let mut sink = TimingSink::default();
    let events = session.run_tool_batch_with_sink(
        &[ToolCallRequest {
            tool_call_id: "call_sleep".to_string(),
            name: "Bash".to_string(),
            input: json!({ "command": "sleep 1" }),
        }],
        &mut sink,
    );

    assert!(
        events.iter().any(|event| matches!(
            event.payload_as::<ToolUpdatedPayload>().expect("能解码"),
            Some(ToolUpdatedPayload::Result(_))
        )),
        "sleep 1 应成功"
    );
    let started_at = sink.started_at.expect("started 已投递");
    let result_at = sink.result_at.expect("result 已投递");
    let elapsed = result_at.duration_since(started_at);
    assert!(
        elapsed >= Duration::from_millis(500),
        "result 必须晚于 started 一个执行窗口（实际 {elapsed:?}）"
    );
}

#[test]
fn task_control_can_run_parallel_even_when_not_read_only() {
    let workspace = TempWorkspace::new("task-stop-parallel");
    let mut session = Session::new("sess_task", workspace.cwd().to_string_lossy().to_string());
    let _ = session.begin_turn("停任务");
    let events = session.run_tool_batch(&[ToolCallRequest {
        tool_call_id: "call_stop".to_string(),
        name: "TaskStop".to_string(),
        input: json!({ "task_id": "exec_missing" }),
    }]);

    let scheduled = events
        .iter()
        .find_map(
            |event| match event.payload_as::<ToolUpdatedPayload>().expect("能解码") {
                Some(ToolUpdatedPayload::Scheduled(payload)) => Some(payload),
                _ => None,
            },
        )
        .expect("有 scheduled");
    assert_eq!(scheduled.tool_name, "TaskStop");
    assert_eq!(
        scheduled.can_run_parallel,
        Some(true),
        "TaskStop 的 canRunParallel 按观察是 true（不取 read_only）"
    );
}
