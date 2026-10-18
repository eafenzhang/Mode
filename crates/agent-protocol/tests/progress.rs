//! `tool.updated` / `kind:"progress"` 的线上形状（M2 Task 5 Step 0 观察）。
//!
//! 样本取自 `docs/specs/tauri-agent-runtime.md` §3.5 的一次实测（长跑 Bash）。

use agent_protocol::session_event::ToolUpdatedPayload;
use serde_json::json;

#[test]
fn progress_payload_round_trips_the_observed_shape() {
    let raw = json!({
        "toolCallId": "toolu_mock_2_0",
        "toolName": "Bash",
        "elapsedMs": 3049,
        "pid": 21596,
        "stdoutBytes": 0,
        "stderrBytes": 0,
        "outputBytes": 0,
        "outputPreview": {
            "text": "",
            "fullText": "",
            "totalLines": 0,
            "totalBytes": 0,
            "linesEstimated": false
        },
        "stdoutTail": "",
        "kind": "progress"
    });

    let payload: ToolUpdatedPayload = serde_json::from_value(raw.clone()).expect("progress 能解码");
    let ToolUpdatedPayload::Progress(progress) = payload else {
        panic!("kind 应判别为 progress");
    };
    assert_eq!(progress.tool_call_id, "toolu_mock_2_0");
    assert_eq!(progress.tool_name, "Bash");
    assert_eq!(progress.elapsed_ms, 3049);
    assert_eq!(progress.pid, 21596);
    assert_eq!(progress.output_preview.total_lines, 0);
    assert!(!progress.output_preview.lines_estimated);

    let encoded = serde_json::to_value(ToolUpdatedPayload::Progress(progress)).expect("可序列化");
    assert_eq!(encoded, raw, "往返后的形状应与观察值逐字一致");
}
