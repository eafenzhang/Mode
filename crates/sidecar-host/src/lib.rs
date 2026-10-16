//! Host sidecar 的**进程监督与字节传输**（M3 Task 1 的岩心，刻意框架无关）。
//!
//! 规格与验收分工：`docs/specs/tauri-renderer-host.md`。路线来源：M3 Task 0 的 spike
//! （`docs/plans/2026-10-16-m3-port-transport-spike.md` §3/§4）——本 crate 只做它选定那条路线的
//! 产品化，不另立机制。纪律：`docs/specs/clean-room-policy.md`（`crates/**` 全部适用，本 crate 新写）。
//!
//! ## 边界：为什么这里没有 Tauri 类型
//!
//! 壳侧的东西（窗口装配、`#[tauri::command]`、`AppHandle` 上的路径解析）在
//! `apps/desktop-tauri/src-tauri` 里。这不是随手分的：只要本 crate 引用**任何** Tauri 类型，
//! 链接面就会把 `wry`/`webview2` 拉进来，而 **cargo 的测试目标没有应用清单**
//! （`tauri-build` 只给 bin 注入）——Windows 上测试二进制会静态导入 `comctl32` +
//! `WebView2Loader.dll`，本地 `cargo test` 直接以 `STATUS_ENTRYPOINT_NOT_FOUND` 起不来
//! （本 crate 的第一版就这么挂过，实测：`target/debug/deps/sidecar_host-*.exe` 的导入表里
//! 同时出现 `comctl32.dll` 与 `WebView2Loader.dll`）。保持框架无关之后：
//!
//! - 进程监督与帧传输的用例能真跑（`tests/host_lifecycle.rs` 会真拉 Node 子进程）；
//! - 壳那侧只剩「把 Tauri 的事件接到本 crate 的 API 上」，没有可测的逻辑。
//!
//! ## 职责
//!
//! - [`process`]：spawn / 监督 / 退出清理（不留孤儿）/ 就绪与失败的单赢家判定；
//! - [`startup`]：发布给渲染层的 `DatabaseStartupState`（壳是唯一发布者，见该模块注释）；
//! - [`frame`]：`[4 字节大端长度][payload]` 帧编解码；
//! - [`bridge`]：注入 webview 的桥脚本模板与占位符替换。

pub mod bridge;
pub mod frame;
pub mod process;
pub mod startup;

pub use process::{HostConfig, HostError, HostHandle};
pub use startup::{StartupErrorCode, StartupPhase, StartupSnapshot};

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// 数据根解析：显式覆写（`MODE_DATA_BASE_DIR`）优先，否则 `<home>/.mode`。
///
/// 与 Node 侧 `packages/desktop/src/host-standalone/dataRoot.ts` 是**同一口径**（测试与本地跑
/// 靠它把 Host 关进临时目录，绝不碰用户真实 `~/.mode`）。这里只做路径决策，不碰文件系统：
/// Task 1 的 Host 不写盘，真存储是 Task 2 的范围。
pub fn data_root_from(override_dir: Option<&OsStr>, home: &Path) -> PathBuf {
    match override_dir {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => home.join(".mode"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_data_root_wins() {
        let root = data_root_from(
            Some(OsStr::new("D:/tmp/isolated")),
            Path::new("C:/Users/someone"),
        );
        assert_eq!(root, PathBuf::from("D:/tmp/isolated"));
    }

    #[test]
    fn falls_back_to_home_dot_mode() {
        let root = data_root_from(None, Path::new("C:/Users/someone"));
        assert_eq!(root, PathBuf::from("C:/Users/someone/.mode"));
        let empty = data_root_from(Some(OsStr::new("")), Path::new("C:/Users/someone"));
        assert_eq!(
            empty,
            PathBuf::from("C:/Users/someone/.mode"),
            "空覆写等于没有覆写"
        );
    }
}
