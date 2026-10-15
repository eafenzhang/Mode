//! 协议类型的 JSON 形状断言（路线图 M1 任务 3 Step 1）。
//!
//! 这些用例钉住的是**线协议**：字段名（camelCase）、`Option` 字段「不出现而非出现为 null」、
//! 判别联合的 `kind` 取值。生成物 `packages/shared/src/generated/tauri-protocol.ts` 由同一份
//! schemars 元数据产出，所以这里过就等于 TS 侧的形状也对得上。
use protocol::{
    ApplicationIconInfo, ApplicationIconLocator, ApplicationIconRequest, OpenPathResult,
    SaveFileRequest, SaveFileRequestError, SaveFileResult, TaskNotificationPayload,
    TaskNotificationStatus,
};

#[test]
fn task_notification_payload_uses_camel_case_and_omits_absent_option() {
    let payload = TaskNotificationPayload {
        task_id: "task-1".to_string(),
        status: TaskNotificationStatus::PermissionRequest,
        request_id: None,
        title: "标题".to_string(),
        body: "正文".to_string(),
    };
    let json = serde_json::to_value(&payload).expect("serialize");
    assert_eq!(
        json,
        serde_json::json!({
            "taskId": "task-1",
            "status": "permission_request",
            "title": "标题",
            "body": "正文",
        }),
        "requestId 为 None 时必须整个字段缺席，不能写成 null"
    );

    let with_request = TaskNotificationPayload {
        request_id: Some("req-9".to_string()),
        ..payload
    };
    let json = serde_json::to_value(&with_request).expect("serialize");
    assert_eq!(json.get("requestId"), Some(&serde_json::json!("req-9")));
}

#[test]
fn task_notification_status_covers_every_contract_value() {
    // 与 packages/shared/src/platform.ts 的联合取值逐一对应；漏一个就会让 UI 收到未知状态。
    let values = [
        (TaskNotificationStatus::Completed, "completed"),
        (TaskNotificationStatus::Failed, "failed"),
        (
            TaskNotificationStatus::PermissionRequest,
            "permission_request",
        ),
        (
            TaskNotificationStatus::ElicitationRequest,
            "elicitation_request",
        ),
        (TaskNotificationStatus::FeedbackUpdate, "feedback_update"),
    ];
    for (status, expected) in values {
        assert_eq!(
            serde_json::to_value(status).expect("serialize"),
            serde_json::json!(expected)
        );
    }
}

#[test]
fn application_icon_locator_is_a_kebab_case_tagged_union() {
    let request = ApplicationIconRequest {
        locators: vec![
            ApplicationIconLocator::WindowsExecutablePath {
                value: "C:\\chrome.exe".to_string(),
            },
            ApplicationIconLocator::DarwinBundleId {
                value: "com.apple.Safari".to_string(),
            },
            ApplicationIconLocator::WindowsAumid {
                value: "Microsoft.WindowsCalculator".to_string(),
            },
        ],
    };
    let json = serde_json::to_value(&request).expect("serialize");
    assert_eq!(
        json,
        serde_json::json!({
            "locators": [
                { "kind": "windows-executable-path", "value": "C:\\chrome.exe" },
                { "kind": "darwin-bundle-id", "value": "com.apple.Safari" },
                { "kind": "windows-aumid", "value": "Microsoft.WindowsCalculator" },
            ]
        })
    );
}

#[test]
fn save_file_request_round_trips_bytes_and_urls() {
    let from_bytes = SaveFileRequest {
        data: Some(vec![1, 2, 3]),
        source_url: None,
        suggested_name: "a.bin".to_string(),
    };
    assert_eq!(from_bytes.validate(), Ok(()));
    assert_eq!(
        serde_json::to_value(&from_bytes).expect("serialize"),
        serde_json::json!({ "data": [1, 2, 3], "suggestedName": "a.bin" })
    );

    let from_url = SaveFileRequest {
        data: None,
        source_url: Some("https://example.invalid/a.png".to_string()),
        suggested_name: "a.png".to_string(),
    };
    assert_eq!(from_url.validate(), Ok(()));
    assert_eq!(
        serde_json::to_value(&from_url).expect("serialize"),
        serde_json::json!({
            "sourceUrl": "https://example.invalid/a.png",
            "suggestedName": "a.png",
        })
    );

    // 反序列化同样要认这两个形状（命令入参走的就是这条路）。
    let parsed: SaveFileRequest = serde_json::from_value(serde_json::json!({
        "sourceUrl": "https://example.invalid/a.png",
        "suggestedName": "a.png",
    }))
    .expect("deserialize");
    assert_eq!(parsed, from_url);
}

#[test]
fn save_file_request_rejects_violating_unions() {
    let neither = SaveFileRequest {
        data: None,
        source_url: None,
        suggested_name: "a.bin".to_string(),
    };
    assert_eq!(
        neither.validate(),
        Err(SaveFileRequestError::MissingPayload)
    );

    let both = SaveFileRequest {
        data: Some(vec![0]),
        source_url: Some("https://example.invalid/a.png".to_string()),
        suggested_name: "a.bin".to_string(),
    };
    assert_eq!(
        both.validate(),
        Err(SaveFileRequestError::ConflictingPayload)
    );
}

#[test]
fn result_shapes_omit_absent_options() {
    // 成功：只有 success + path；`error` 必须缺席。
    let saved = SaveFileResult {
        canceled: None,
        error: None,
        path: Some("D:\\out.bin".to_string()),
        success: true,
    };
    assert_eq!(
        serde_json::to_value(&saved).expect("serialize"),
        serde_json::json!({ "path": "D:\\out.bin", "success": true })
    );

    // 取消：canceled + success=false，且**没有** path。
    let canceled = SaveFileResult {
        canceled: Some(true),
        error: None,
        path: None,
        success: false,
    };
    assert_eq!(
        serde_json::to_value(&canceled).expect("serialize"),
        serde_json::json!({ "canceled": true, "success": false })
    );

    let failed = OpenPathResult {
        success: false,
        error: Some("path_unavailable".to_string()),
    };
    assert_eq!(
        serde_json::to_value(&failed).expect("serialize"),
        serde_json::json!({ "success": false, "error": "path_unavailable" })
    );

    let icon = ApplicationIconInfo {
        icon_data_url: "data:image/png;base64,AA==".to_string(),
    };
    assert_eq!(
        serde_json::to_value(&icon).expect("serialize"),
        serde_json::json!({ "iconDataUrl": "data:image/png;base64,AA==" })
    );
}
