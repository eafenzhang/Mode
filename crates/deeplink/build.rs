//! 给**测试目标**注入 Windows 应用清单（comctl32 v6 依赖）。
//!
//! 本 crate 依赖 tauri（发事件、用 deep-link 插件），因此 `tests/intent.rs` 的测试二进制
//! 会被链入 tao 的 Windows GUI 代码，不带清单时启动即
//! `STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139)`。机制详见
//! `crates/tauri-test-manifest/src/lib.rs`。
fn main() {
    tauri_test_manifest::emit_for_tests("deeplink");
}
