//! 给**测试目标**注入 Windows 应用清单（comctl32 v6 依赖）。
//!
//! 与 `crates/platform/build.rs` 同一个理由与同一份实现：本 crate 的单测会链接 tauri，
//! 而 tauri 的 Windows 路径用到 comctl32 v6 的导出；测试二进制默认没有清单，会在启动阶段
//! `STATUS_ENTRYPOINT_NOT_FOUND`。机制见 `crates/tauri-test-manifest/src/lib.rs`。
fn main() {
    tauri_test_manifest::emit_for_tests("updater");
}
