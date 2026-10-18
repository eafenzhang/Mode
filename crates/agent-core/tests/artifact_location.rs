//! M2 Task 4：`Bash` 大输出 artifact 的**会话落点**接线。
//!
//! 期望路径来自 oracle 观察（`docs/specs/tauri-agent-runtime.md` §3.4）：
//! `<dataRoot>/.mode/cli/exec/<sessionId>/<toolCallId>-stdout.log`。
//!
//! 本用例只钉住「会话把数据根与调用 id 传到了工具」，不重录夹具——
//! 夹具刻意不覆盖 >30KB（路径含会话 id，录进夹具不可复现）。

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use agent_core::session::{Session, ToolCallRequest};
use agent_core::{SessionEvent, SessionEventType, ToolUpdatedPayload};
use serde_json::json;

fn temp_base(tag: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "mode-artifact-{tag}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&base).expect("建临时基目录");
    base
}

/// 取工具成功结果的正文（与对拍投影同一字段）。
fn result_content(events: &[SessionEvent]) -> String {
    for event in events {
        if event.event_type != SessionEventType::ToolUpdated {
            continue;
        }
        let Some(payload) = event
            .payload_as::<ToolUpdatedPayload>()
            .expect("载荷能解码")
        else {
            continue;
        };
        if let ToolUpdatedPayload::Result(result) = payload {
            return result
                .result
                .content
                .as_ref()
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
        }
    }
    panic!("事件流里没有 result");
}

#[test]
fn bash_artifact_lands_in_the_session_exec_dir() {
    let base = temp_base("location");
    // `data_root` 是已解析的 `.mode` 根（`{base}/.mode`），不是基目录本身。
    let data_root = base.join(".mode");
    let mut session = Session::new("sess_art", base.to_string_lossy().to_string())
        .with_data_root(data_root.clone());

    session.begin_turn("artifact");
    let events = session.run_tool_batch(&[ToolCallRequest {
        tool_call_id: "call_art".to_string(),
        name: "Bash".to_string(),
        input: json!({ "command": "head -c 31000 /dev/zero | tr '\\0' 'x'" }),
    }]);

    let expected = data_root
        .join("cli")
        .join("exec")
        .join("sess_art")
        .join("call_art-stdout.log");
    assert!(
        expected.exists(),
        "artifact 应落在 {expected:?}（观察值 <dataRoot>/.mode/cli/exec/<sessionId>/<toolCallId>-stdout.log）"
    );
    assert_eq!(fs::read(&expected).expect("读 artifact").len(), 31_000);
    assert!(
        result_content(&events).contains(&expected.display().to_string()),
        "包装里应写出真实落点"
    );

    let _ = fs::remove_dir_all(&base);
}
