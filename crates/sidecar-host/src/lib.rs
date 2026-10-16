//! Host sidecar 的**进程监督与字节传输**（M3 Task 1 的岩心，刻意框架无关）。
//!
//! 规格与验收分工：`docs/specs/tauri-renderer-host.md`。路线来源：M3 Task 0 的 spike
//! （`docs/plans/2026-10-16-m3-port-transport-spike.md` §3/§4）——本 crate 只做它选定那条路线的
//! 产品化，不另立机制。纪律：`docs/specs/clean-room-policy.md`（`crates/**` 全部适用，本 crate 新写）。
//!
//! ## 边界：为什么这里没有 Tauri 类型
//!
//! 壳侧的东西（窗口装配、`#[tauri::command]`、`AppHandle` 上的候选枚举）在
//! `apps/desktop-tauri/src-tauri` 里。理由只写**实测到的**（复评 I5 要求不要把已解决的问题
//! 说成不可避免的）：
//!
//! 1. 本 crate 的第一版把窗口装配与命令面也放在里面，`cargo test -p sidecar-host` 的测试二进制
//!    以 `STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139)` 起不来，`objdump -p` 显示它同时导入了
//!    `comctl32.dll` 与 `WebView2Loader.dll`（对照 `crates/platform` 的测试二进制：两者都没有，
//!    跑得起来）；
//! 2. `crates/tauri-test-manifest`（comctl32 v6 测试清单注入）**当时已经接上**——build script
//!    发出了 `rustc-link-arg-tests`、`out/*.coff.o` 里也有 `.rsrc` 段——但**链出来的测试 exe 里
//!    没有 `.rsrc` 段**，即那份清单在这个目标上没有生效。**机制未查明**；
//! 3. `WebView2Loader.dll` 那一半同样未查明（把 `target/debug/WebView2Loader.dll` 拷到
//!    `target/debug/deps/` 旁边也没解决）。
//!
//! 所以这不是「Tauri 类型一律不能用」：`platform` 同样引用 Tauri。去掉**具体 runtime**引用后
//! （`AppHandle` 走泛型、窗口装配移出）测试二进制不再导入那两个 DLL、用例能真跑。
//! 代价照记：`mode_tauri::host::{resolve_config, resolve_entry}` 因此落在 app crate、无自动化覆盖
//! （见 `docs/specs/tauri-renderer-host.md` 的覆盖分工表），能抽成纯函数的部分
//! （[`pick_entry`]、[`data_base_dir_from`]）已经抽进来并有用例。
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

/// 数据**基**目录解析：显式覆写（`MODE_DATA_BASE_DIR`）优先，否则家目录。
///
/// **`MODE_DATA_BASE_DIR` 是基目录，不是数据根**：数据根 = `{base}/.mode`。
/// 口径来自仓库既有实现（`packages/services/src/paths.ts` 的 `getDataBaseDir()` /
/// `getModeDataRootDir()`：`setDataBaseDir` 的注释原文是 "replaces homedir() prefix"）。
/// Node 侧同一口径见 `packages/desktop/src/host-standalone/dataRoot.ts`。
///
/// 2026-10-16 的复评钉过这条：把基目录当根用，真服务会往 `<base>/.mode/.mode` 落盘——
/// Task 1 不写盘所以看不出来，Task 2 接真存储时会写到错的地方。
pub fn data_base_dir_from(override_base: Option<&OsStr>, home: &Path) -> PathBuf {
    match override_base {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => home.to_path_buf(),
    }
}

/// 数据根 = `{base}/.mode`（`MODE_DATA_ROOT_DIR_NAME`，与 `paths.ts` 同名）。
pub fn data_root_from_base(base: &Path) -> PathBuf {
    base.join(".mode")
}

/// 入口脚本挑选（纯函数，**不含 Tauri 类型**，因此可以被用例覆盖）。
///
/// 规则两条，都是刻意的：
///
/// 1. 显式覆写（`MODE_HOST_SIDECAR_ENTRY`）**优先，且不做兜底**：路径写错了就直接没有入口，
///    由壳发布 `failed(unsupported_runtime)`。静默回落到别的入口会让「配置错」变成
///    「跑的是另一个 Host」，那种失败最难查；
/// 2. 没有覆写时按 `candidates` 顺序取**第一个存在的**（资源目录档 → debug 构建的仓库相对档）。
///
/// 空字符串视同未设置（与 `MODE_DATA_BASE_DIR` 同口径）。
pub fn pick_entry(env_override: Option<&OsStr>, candidates: &[PathBuf]) -> Option<PathBuf> {
    if let Some(explicit) = env_override.filter(|value| !value.is_empty()) {
        let path = PathBuf::from(explicit);
        return path.exists().then_some(path);
    }
    candidates.iter().find(|candidate| candidate.exists()).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_base_dir_wins_and_root_is_base_dot_mode() {
        let base = data_base_dir_from(
            Some(OsStr::new("D:/tmp/isolated")),
            Path::new("C:/Users/someone"),
        );
        assert_eq!(base, PathBuf::from("D:/tmp/isolated"));
        assert_eq!(
            data_root_from_base(&base),
            PathBuf::from("D:/tmp/isolated/.mode"),
            "根是 base/.mode：覆写的是基目录，不是根"
        );
    }

    #[test]
    fn falls_back_to_home_and_empty_is_treated_as_unset() {
        let home = Path::new("C:/Users/someone");
        assert_eq!(data_base_dir_from(None, home), PathBuf::from("C:/Users/someone"));
        assert_eq!(
            data_root_from_base(&data_base_dir_from(None, home)),
            PathBuf::from("C:/Users/someone/.mode")
        );
        assert_eq!(
            data_base_dir_from(Some(OsStr::new("")), home),
            PathBuf::from("C:/Users/someone"),
            "空覆写等于没有覆写"
        );
    }

    #[test]
    fn explicit_entry_wins_and_missing_explicit_entry_does_not_fall_back() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let existing = manifest.join("Cargo.toml");
        let missing = manifest.join("no-such-entry.ts");

        let candidates = std::slice::from_ref(&existing);
        assert_eq!(
            pick_entry(Some(existing.as_os_str()), candidates),
            Some(existing.clone()),
            "显式覆写优先于候选列表"
        );
        assert_eq!(
            pick_entry(Some(missing.as_os_str()), candidates),
            None,
            "显式覆写指向不存在的路径时必须**没有入口**，不得静默回落到别的入口"
        );
        assert_eq!(
            pick_entry(Some(OsStr::new("")), candidates),
            Some(existing),
            "空覆写等于没有覆写"
        );
    }

    #[test]
    fn entry_candidates_are_scanned_in_order_and_missing_ones_are_skipped() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let existing = manifest.join("Cargo.toml");
        assert_eq!(
            pick_entry(
                None,
                &[PathBuf::from("missing-1"), existing.clone(), existing.clone()]
            ),
            Some(existing.clone()),
            "没有覆写时取第一个存在的候选"
        );
        assert_eq!(pick_entry(None, &[PathBuf::from("missing-1")]), None);
    }
}
