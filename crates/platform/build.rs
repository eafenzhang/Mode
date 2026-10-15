//! 给**测试目标**注入 Windows 应用清单（comctl32 v6 依赖）。
//!
//! 具体机制、以及「为什么不能用 `tauri-winres` / `embed-resource`」的完整因果，
//! 见 `crates/tauri-test-manifest/src/lib.rs` 的模块文档——这段逻辑已抽成构建依赖，
//! 因为依赖 tauri 的 crate 会越来越多，抄多份等于把同一个坑复制多份。
//!
//! 这里只保留本 crate 特有的一条：**测试必须留在 `tests/`（集成测试目标）**。
//! `cargo:rustc-link-arg-tests` 只对测试目标生效，而 cargo 把 `tests/` 下的目标
//! 以及 lib 的单测目标都算作测试目标；把 `tests/commands.rs` 挪进 lib 单测模块会同时
//! 丢掉 `tests/` 目标（报 "The package … does not have a test target"）。
fn main() {
    tauri_test_manifest::emit_for_tests("platform");
}
