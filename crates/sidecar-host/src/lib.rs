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
//! - [`host_stderr`]：Host stderr 的留痕与**结构化失败事实**抽取（DR-T3-3 ①：把「准备任务索引
//!   时存储打开失败」这类阶段精确的原因送到渲染层，而不是塌成粗粒度 `transport_closed`）；
//! - [`startup`]：发布给渲染层的 `DatabaseStartupState`（壳是唯一发布者，见该模块注释）；
//! - [`frame`]：`[4 字节大端长度][payload]` 帧编解码；
//! - [`bridge`]：注入 webview 的桥脚本模板（含 `window.mode` 的平台面适配层）与占位符替换。

pub mod bridge;
pub mod frame;
pub mod host_stderr;
pub mod process;
pub mod settings_probe;
pub mod startup;

pub use process::{HostConfig, HostError, HostHandle};
pub use startup::{StartupErrorCode, StartupPhase, StartupSnapshot, StartupStage};

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
    candidates
        .iter()
        .find(|candidate| candidate.exists())
        .cloned()
}

/// Node 可执行文件的挑选（纯函数，因此可被用例覆盖）。
///
/// 三档，**优先级是刻意的**：
///
/// 1. `MODE_HOST_NODE` 显式覆写**原样生效**（不做存在性检查）——与 [`pick_entry`] 的
///    「显式覆写不做兜底」不同，这里的覆写只是一个可执行文件路径：写错了照样由
///    `HostHandle::spawn` 以 `open_failed` 报出来（**响亮地失败**），所以静态检查在这里
///    没有额外收益，反而会让「PATH 上的 `node`」这种合法值被误判。
/// 2. **随包 Node**（打包形态，M3 Task 5）：资源目录下的 `node/node.exe`（Windows）或
///    `node/node`。存在即用——它是安装包自带的那一个，用户机器上有没有 Node 与它无关
///    （DR-M3-2：「Host 随安装包发布」，全新安装 + 断网也必须能起 UI）。
/// 3. 退回裸名 `node`：交给 PATH 解析。这是**开发形态**的路径（仓库里跑壳时用的就是它）。
///
/// 为什么单独抽一个函数：候选枚举需要 `AppHandle`，而 `AppHandle` 会让本 crate 的测试二进制
/// 起不来（见模块文档），所以「选哪一个」这条规则必须留在能跑用例的这一侧，
/// 与 [`pick_entry`] / [`pick_shell_entry`] 同一种做法。
pub fn pick_node(env_override: Option<&OsStr>, bundled_candidates: &[PathBuf]) -> PathBuf {
    if let Some(explicit) = env_override.filter(|value| !value.is_empty()) {
        return PathBuf::from(explicit);
    }
    bundled_candidates
        .iter()
        .find(|candidate| candidate.exists())
        .cloned()
        .unwrap_or_else(|| PathBuf::from("node"))
}

/// 去掉 Windows 扩展长度（verbatim）前缀，得到 Node 能解析的普通路径形态。
///
/// ## 为什么必须在起子进程**之前**做
///
/// 打包形态的入口来自 `app.path().resource_dir()`，而它的实现是
/// `tauri_utils::platform::current_exe()` → `tauri-utils-2.10.2/src/platform/starting_binary.rs`
/// 里的 `Self(dangerous_path.canonicalize())`。Windows 上 `Path::canonicalize()` 返回的正是
/// **verbatim 形态** `\\?\C:\…`（UNC 则是 `\\?\UNC\server\share`）。
///
/// 把这种路径当**入口脚本**传给 node，Node 24 的主模块解析（`resolveMainPath` →
/// `Module._findPath` → `fs.realpathSync`）解不了它，进程在**用户代码之前**就以
/// `EISDIR: illegal operation on a directory, lstat 'C:'` 退出。后果是 Host 连
/// `startup_failed` 那一行都来不及写（那一行在 `index.ts` 的 try/catch 里，而节点根本没走到
/// 装载入口），壳只能报粗粒度的 `transport_closed` / `failedPhase=starting`。
///
/// 实测（同一份已打包的 `host-sidecar/index.mjs`，只换入口路径形态）：
/// 普通绝对路径 → Host 就绪；`\\?\` 前缀 → node 退出码 1、约 100ms 即死。
/// 详见 `.superpowers/sdd/2026-10-16-tauri-m3-ecosystem/host-startup-failure-diagnosis.md`。
///
/// ## 只认 verbatim 两种形态，其余原样返回
///
/// - `\\?\UNC\server\share` → `\\server\share`
/// - `\\?\C:\…` → `C:\…`
///
/// 非 Windows 平台上 `\\?\` 不是特殊前缀，但这两种形态也不会出现，函数等价于恒等。
/// 非 UTF-8 路径原样返回（不做有损改写）。
///
/// 已知代价：verbatim 前缀同时关闭了路径规范化，也是**超过 260 字符**长路径的唯一写法；
/// 去掉之后超长路径会退回传统上限。当前安装落点远短于该上限，先以「node 能起」为准。
pub fn strip_verbatim_prefix(path: &Path) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path.to_path_buf();
    };
    if let Some(rest) = text
        .get(..8)
        .filter(|head| head.eq_ignore_ascii_case(r"\\?\UNC\"))
        .map(|_| &text[8..])
    {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    path.to_path_buf()
}

/// 壳加载的**前端入口页**（webview 的 `WebviewUrl::App(…)`）挑选：纯函数，因此可被用例覆盖。
///
/// 产品页是默认（`index.html`，真 renderer 的构建产物）。M1 的命令面验收台按 DR-T3-2
/// 保留为**开发/诊断入口**：它是对拍与命令面回归的工具，不能一删了之，但也不该进发布产物
/// （多占体积、并把内部命令面暴露出去）。所以它由 `MODE_TAURI_SHELL_ENTRY` 显式切换
/// ——开发跑 `MODE_TAURI_SHELL_ENTRY=dev/index.html` 就能进验收台。
///
/// 白名单两条，理由：
///
/// 1. 只接受**相对**路径（不含盘符/前导分隔符/`..`）：这个值会直接进 `WebviewUrl::App`，
///    允许绝对路径或上跳就等于把「壳加载哪个页面」变成任意本地文件读取；
/// 2. 空值视同未设置（与 `MODE_DATA_BASE_DIR` / `MODE_HOST_SIDECAR_ENTRY` 同口径）。
pub fn pick_shell_entry(env_override: Option<&OsStr>) -> PathBuf {
    let default = Path::new("index.html");
    let Some(raw) = env_override.filter(|value| !value.is_empty()) else {
        return default.to_path_buf();
    };
    let Some(text) = raw.to_str() else {
        return default.to_path_buf();
    };
    if text.starts_with('/') || text.starts_with('\\') || text.contains(':') || text.contains("..")
    {
        return default.to_path_buf();
    }
    PathBuf::from(text)
}

/// 壳窗口**要不要系统的边框与标题栏**（`WebviewWindowBuilder::decorations` 的取值）。
///
/// **语义容易搞反，所以它在这里、且有用例**：`decorations(true)` = **有**系统标题栏
/// （`tauri-2.12.4` 的 `webview_window.rs` → `tauri-runtime-wry` → `tao` 的
/// `with_decorations`，契约是「要不要边框与标题栏」）。而产品 UI **自己画标题栏**
/// （`packages/ui` 的 `DesktopWindowControls` + `[app-region:drag]` 区域），所以：
///
/// | 入口 | 覆写 | 返回值 | 为什么 |
/// | --- | --- | --- | --- |
/// | 产品页（`index.html`，真 renderer 产物） | 未覆写 | **`false`（无边框）** | 否则用户看到**两条**标题栏：系统的那条 + 自绘的那条 |
/// | 开发/诊断入口（验收台）与集成测试页 | 覆写 / 显式指定 | **`true`（保留系统栏）** | 那两页没有自绘标题栏，去掉系统栏会让它们既移不动也关不掉 |
///
/// 与 [`pick_shell_entry`] 同一个参数（`MODE_TAURI_SHELL_ENTRY` 的原始值），所以两处判据不可能分叉。
///
/// **这曾经被写反过**（`!entry_overridden` 直接进 `decorations`）：产品窗口两条标题栏、
/// 验收台反而无边框——用户报的就是前一半。纯函数 + 两个方向的用例就是让那种反转在 CI 里红。
pub fn decorations_for_shell_entry(env_override: Option<&OsStr>) -> bool {
    // 空值视同未设置：与 `pick_shell_entry` 同口径（不然 `MODE_TAURI_SHELL_ENTRY=` 会让
    // 产品窗口变成有边框、或让验收台变成无边框——两种都错）。
    env_override.filter(|value| !value.is_empty()).is_some()
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
        assert_eq!(
            data_base_dir_from(None, home),
            PathBuf::from("C:/Users/someone")
        );
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
                &[
                    PathBuf::from("missing-1"),
                    existing.clone(),
                    existing.clone()
                ]
            ),
            Some(existing.clone()),
            "没有覆写时取第一个存在的候选"
        );
        assert_eq!(pick_entry(None, &[PathBuf::from("missing-1")]), None);
    }

    /// 无边框策略：**产品入口无边框、开发/诊断入口保留系统栏**（两个方向都钉住）。
    ///
    /// 这条用例是「用户报障」的回归网：把判据写反（`!entry_overridden`）产品窗口就会有两条
    /// 标题栏，而验收台会变成一块移不动也关不掉的板子——两个方向都在这里红。
    #[test]
    fn only_the_product_entry_is_frameless() {
        assert!(
            !decorations_for_shell_entry(None),
            "产品入口（未覆写）必须是**无边框**：自绘标题栏是产品 UI 唯一的那条栏"
        );
        assert!(
            !decorations_for_shell_entry(Some(OsStr::new(""))),
            "空覆写等于没有覆写（与 pick_shell_entry 同口径）→ 仍是产品入口 → 无边框"
        );
        assert!(
            decorations_for_shell_entry(Some(OsStr::new("dev/index.html"))),
            "验收台（开发/诊断入口）必须**保留系统栏**：它没有自绘标题栏，去掉会移不动也关不掉"
        );
    }

    /// 随包 Node 的挑选：**显式覆写 → 资源目录里的随包 node → PATH 上的裸名 `node`**。
    ///
    /// 三档都要钉住，因为漏掉任何一档都会让「安装包里到底跑的是哪个 Node」变成两说：
    /// 打包形态若静默退回 PATH，用户机器上有没有 Node 就决定了 UI 能不能起
    /// （DR-M3-2 要求 Host 随安装包发布）。
    #[test]
    fn node_resolution_prefers_override_then_bundled_then_path() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let bundled = manifest.join("Cargo.toml"); // 用一个真实存在的文件当「随包 node」

        assert_eq!(
            pick_node(
                Some(OsStr::new("D:/custom/node.exe")),
                std::slice::from_ref(&bundled)
            ),
            PathBuf::from("D:/custom/node.exe"),
            "显式覆写优先，且原样生效（合法路径不因不存在被改写）"
        );
        assert_eq!(
            pick_node(None, std::slice::from_ref(&bundled)),
            bundled,
            "没有覆写时用资源目录里那个随包 Node"
        );
        assert_eq!(
            pick_node(None, &[manifest.join("no-such-node.exe")]),
            PathBuf::from("node"),
            "随包 Node 不存在时退回裸名 node（开发形态：PATH 解析）"
        );
        assert_eq!(
            pick_node(Some(OsStr::new("")), std::slice::from_ref(&bundled)),
            bundled,
            "空覆写等于没有覆写（与 pick_entry / data_base_dir_from 同口径）"
        );
    }

    #[test]
    fn shell_entry_defaults_to_the_product_page_and_only_allows_relative_overrides() {
        assert_eq!(
            pick_shell_entry(None),
            PathBuf::from("index.html"),
            "默认必须是产品页（真 renderer 的产物），而不是验收台"
        );
        assert_eq!(
            pick_shell_entry(Some(OsStr::new("dev/index.html"))),
            PathBuf::from("dev/index.html"),
            "验收台按 DR-T3-2 作开发入口：相对路径的覆写要生效"
        );
        assert_eq!(
            pick_shell_entry(Some(OsStr::new(""))),
            PathBuf::from("index.html"),
            "空覆写等于没有覆写"
        );
        // 这个值会直接进 `WebviewUrl::App`：绝对路径与上跳都不接受，一律退回产品页。
        for hostile in [
            "C:/Windows/win.ini",
            "/etc/passwd",
            "\\\\server\\share",
            "../../secret.html",
        ] {
            assert_eq!(
                pick_shell_entry(Some(OsStr::new(hostile))),
                PathBuf::from("index.html"),
                "{hostile} 不是合法的壳入口，必须退回产品页"
            );
        }
    }

    /// verbatim 前缀的剥离：**这是打包形态 Host 起不来的那个缺陷的回归网**。
    ///
    /// 事实链（数值与命令见 `host-startup-failure-diagnosis.md`）：`resource_dir()` 内部
    /// `canonicalize()` → `\\?\C:\…` → 当入口传给 node → node 的 CJS 主模块解析
    /// `realpathSync` 报 `EISDIR: lstat 'C:'` → 约 100ms 退出 → 壳只能报
    /// `transport_closed`/`starting`。所以这两种 verbatim 形态必须被剥成普通路径，
    /// 而**普通路径必须原样通过**（不能顺手改写出别的路径形态）。
    #[test]
    fn verbatim_windows_prefixes_are_stripped_and_normal_paths_pass_through() {
        assert_eq!(
            strip_verbatim_prefix(Path::new(
                r"\\?\C:\Users\someone\install\host-sidecar\index.mjs"
            )),
            PathBuf::from(r"C:\Users\someone\install\host-sidecar\index.mjs"),
            "驱动器形态：剥掉 \\\\?\\ 之后就是普通绝对路径"
        );
        assert_eq!(
            strip_verbatim_prefix(Path::new(r"\\?\UNC\server\share\index.mjs")),
            PathBuf::from(r"\\server\share\index.mjs"),
            "UNC 形态：\\\\.UNC. 要还原成 \\\\ 开头，不是变成相对路径"
        );
        assert_eq!(
            strip_verbatim_prefix(Path::new(r"\\?\unc\server\share\index.mjs")),
            PathBuf::from(r"\\server\share\index.mjs"),
            "UNC 关键字大小写不敏感（Windows 上两种写法都出现过）"
        );
        // 普通路径不得被改写：多剥或少剥都会让「壳决定跑哪个入口」变成两说。
        for untouched in [
            r"C:\Users\someone\install\host-sidecar\index.mjs",
            r"\\server\share\index.mjs",
            "/usr/lib/mode/host-sidecar/index.mjs",
            "packages/desktop/src/host-standalone/index.ts",
            r"D:\ZCodium",
        ] {
            assert_eq!(
                strip_verbatim_prefix(Path::new(untouched)),
                PathBuf::from(untouched),
                "{untouched} 不是 verbatim 形态，必须原样通过"
            );
        }
    }
}
