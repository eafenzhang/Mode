//! 打印桌面任务库的 schema 快照（与 TS `buildTasksSchemaSnapshot()` 同形状）。
//!
//! 用途：`scripts/check-tasks-schema-parity.ts` 在 CI 里把它与 TS 快照逐字段比较，
//! 防止 Rust 迁移与 TS 迁移漂移。输出是紧凑 JSON（单行），只写 stdout。

use std::process::ExitCode;

fn main() -> ExitCode {
    match tasks_db::build_tasks_schema_snapshot() {
        Ok(snapshot) => match serde_json::to_string(&snapshot) {
            Ok(text) => {
                println!("{text}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("tasks-db-snapshot: serialize failed: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("tasks-db-snapshot: {error}");
            ExitCode::FAILURE
        }
    }
}
