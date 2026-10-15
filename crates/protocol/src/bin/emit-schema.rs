//! 打印本 crate 全部契约类型的 JSON Schema（schemars 产物）。
//!
//! 用途：`scripts/generate-tauri-protocol.mjs` 消费这份 JSON 生成
//! `packages/shared/src/generated/tauri-protocol.ts`。导出方向是**单向**的 Rust → TS：
//! Rust 是唯一真源，TS 侧不反向生成，避免两边都能改而互相漂移。
//!
//! 输出格式固定为 `{ "$constants": {...}, "<类型名>": <RootSchema>, ... }` 的 JSON。
//! `$constants` 是与类型同源、但**不是类型**的跨端字面量（目前只有深链事件名）——
//! 它也必须由生成器导出，否则「事件名写错」这类静默失效就没人兜。
use serde_json::json;

fn main() {
    let bundle = json!({
        // 非类型字面量：TS 侧要用的同一个字符串。见模块文档。
        "$constants": {
            "DEEP_LINK_EVENT": protocol::DEEP_LINK_EVENT,
        },
        "DeepLinkEventKind": schemars::schema_for!(protocol::DeepLinkEventKind),
        "DeepLinkEvent": schemars::schema_for!(protocol::DeepLinkEvent),
        "TaskNotificationStatus": schemars::schema_for!(protocol::TaskNotificationStatus),
        "TaskNotificationPayload": schemars::schema_for!(protocol::TaskNotificationPayload),
        "ApplicationIconLocator": schemars::schema_for!(protocol::ApplicationIconLocator),
        "ApplicationIconRequest": schemars::schema_for!(protocol::ApplicationIconRequest),
        "ApplicationIconInfo": schemars::schema_for!(protocol::ApplicationIconInfo),
        "SaveFileRequest": schemars::schema_for!(protocol::SaveFileRequest),
        "SaveFileResult": schemars::schema_for!(protocol::SaveFileResult),
        "OpenPathResult": schemars::schema_for!(protocol::OpenPathResult),
    });
    match serde_json::to_string_pretty(&bundle) {
        Ok(text) => println!("{text}"),
        Err(error) => {
            eprintln!("emit-schema: 序列化 schema 失败：{error}");
            std::process::exit(1);
        }
    }
}
