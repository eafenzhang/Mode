//! 打印本 crate 全部契约类型的 JSON Schema（schemars 产物）。
//!
//! 用途：`scripts/generate-tauri-protocol.mjs` 消费这份 JSON 生成
//! `packages/shared/src/generated/tauri-protocol.ts`。导出方向是**单向**的 Rust → TS：
//! Rust 是唯一真源，TS 侧不反向生成，避免两边都能改而互相漂移。
//!
//! 输出格式固定为 `{ "<类型名>": <RootSchema>, ... }` 的 JSON，键顺序即 TS 声明顺序。
use serde_json::json;

fn main() {
    let bundle = json!({
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
