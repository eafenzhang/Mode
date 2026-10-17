//! 更新链（路线图 M1 任务 6 的最小面）。
//!
//! 规格：`docs/specs/tauri-platform-commands.md` §5。
//!
//! ## 本阶段到什么程度
//!
//! 与旧 Electron 线**共用同一个 GitHub Releases feed**，版本号延续。两套清单格式并存于
//! 同一个 release：
//!
//! - `latest.yml` + `*.blockmap` —— 旧 Electron 线（`electron-updater`）在用；
//! - `latest.json` —— Tauri updater 的清单格式；M5 Task 1 起由发布构建产出并随 Release 上传。
//!
//! M1 只声明端点与注册插件；M5 Task 1 补上真实密钥、`latest.json` 与**可执行触发**
//! （[`command::check_and_install`]，由壳的 `executeDesktopCommand("checkForUpdates")` 调用）。
//! 触发语义仍是 fail closed：签名或清单不对就返回错误，旧版本继续运行。
//!
//! ## 为什么端点是常量而不是配置项
//!
//! 旧线把 feed 写在打包配置里（`packages/desktop/electron-builder.config.js` 的
//! `publish`），那是**打包期**的事。Tauri 侧同一个 feed 是**运行期**读的，写死成常量
//! 有两条好处：一是单测能直接断言端点字符串，二是「两条线必须指向同一个 release」
//! 这件事在代码里可读，而不是散在构建脚本里。

pub mod command;

/// GitHub Releases feed 的所有者（与旧线 `electron-builder.config.js` 的 `publish.owner` 一致）。
pub const RELEASE_FEED_OWNER: &str = "eafenzhang";
/// GitHub Releases feed 的仓库名（与旧线 `publish.repo` 一致）。
pub const RELEASE_FEED_REPO: &str = "Mode";
/// Tauri updater 清单文件名（`electron-updater` 的 `latest.yml` 是另一个资产，不冲突）。
pub const TAURI_MANIFEST_ASSET: &str = "latest.json";

/// Tauri updater 的清单端点。
///
/// 走 `releases/latest/download/` 而不是 `releases/download/<tag>/`：前者由 GitHub 按
/// 「最新正式 release」解析，与旧线 `electron-updater` 默认取 latest 的口径一致，
/// 不需要在每次发版时改代码。
pub fn manifest_endpoint() -> String {
    format!(
        "https://github.com/{RELEASE_FEED_OWNER}/{RELEASE_FEED_REPO}/releases/latest/download/{TAURI_MANIFEST_ASSET}"
    )
}

/// 组装 updater 插件。
///
/// ## 配置必须写在 `tauri.conf.json`，且 `pubkey` 不能省（**实测过的坑**）
///
/// 插件通过 `tauri_plugin_updater::Config` 反序列化 `plugins.updater` 段。该结构体的
/// `pubkey: String` **没有 `#[serde(default)]`**，于是：
///
/// - 整段缺席 → Tauri 传 `null` → `PluginInitialization("updater", "invalid type: null,
///   expected struct Config")`，**app 启动即 panic**；
/// - 段存在但少了 `pubkey` 键 → `missing field pubkey`，同样起不来。
///
/// 所以「不设 pubkey」这件事**做不到字面意思**：字段必须在。M1–M4 期间取值是**空串**，
/// 语义上等价于「没有密钥」——空串走到 `verify_signature` 里会在
/// `base64_to_string("")` → `PublicKey::decode("")` 处报错，**fail closed**，
/// 不会静默放行未签名的更新（已核对 `tauri-plugin-updater-2.14.0/src/updater.rs:1901`）。
/// M5 Task 1 起换成真实的 minisign 公钥：语义从「因为没有密钥而拒绝一切」变成
/// 「用这把密钥校验签名」。**空串仍然是 fail closed**：任何不带 pubkey 的构建
/// （例如有人从模板重建 conf）都更新不了，这是刻意保留的。
/// 公钥只是 `.pub` 文件的 base64；私钥与密码只在 CI secrets 与本机仓库外。
///
/// 端点同时写在 `crates/updater` 的 [`manifest_endpoint`] 与 conf 两处，原因见
/// `the_app_conf_endpoint_matches_the_constant`：`Builder` 没有 `endpoints()`，
/// 端点只能从配置来，常量是给代码与单测看的那份；两者由测试钉死，改一处即红。
pub fn plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R, tauri_plugin_updater::Config> {
    tauri_plugin_updater::Builder::new().build()
}
