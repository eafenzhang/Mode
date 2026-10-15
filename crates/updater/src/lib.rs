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
//! - `latest.json` —— Tauri updater 的清单格式，**M1 起声明端点**，但 Release 里
//!   目前还没有这个资产。
//!
//! 「声明端点」与「真的能更新」是两件事，这里只做前者：插件注册好、端点常量定死，
//! 实际发布 `latest.json`、密钥签名与三平台更新演练属 M5（路线图「发布工程」）。
//! 因此**不注册任何更新相关的 `#[tauri::command]`**——渲染层在 M1 无法触发更新，
//! 也就不会有「调了没反应」的半成品（更新相关的命令面在 M3/M5 与侧车一起接）。
//!
//! ## 为什么端点是常量而不是配置项
//!
//! 旧线把 feed 写在打包配置里（`packages/desktop/electron-builder.config.js` 的
//! `publish`），那是**打包期**的事。Tauri 侧同一个 feed 是**运行期**读的，写死成常量
//! 有两条好处：一是单测能直接断言端点字符串，二是「两条线必须指向同一个 release」
//! 这件事在代码里可读，而不是散在构建脚本里。

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
/// **M1 不设 pubkey**：签名密钥与三平台签名流水线属 M5。没有 pubkey 时插件的
/// `check()` 会明确报错（而不是静默放行未签名的更新），这正是想要的行为——
/// 骨架期不可能被误当成「更新链已可用」。
///
/// 返回类型的第二个参数是插件自己的配置类型（`tauri_plugin_updater::Config`），
/// 它由 `tauri.conf.json` 的 `plugins.updater` 段提供；M1 该段刻意留空，
/// 因为端点与签名密钥都还没定稿。
pub fn plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R, tauri_plugin_updater::Config> {
    tauri_plugin_updater::Builder::new().build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_endpoint_points_at_the_shared_release_feed() {
        assert_eq!(
            manifest_endpoint(),
            "https://github.com/eafenzhang/Mode/releases/latest/download/latest.json"
        );
    }

    #[test]
    fn the_two_lines_share_one_feed_but_not_one_manifest_file() {
        // 共用 feed 的前提是「两个清单文件互不覆盖」：Tauri 用 latest.json，
        // 旧线用 latest*.yml。这条断言把该前提写下来，改名即红。
        assert!(TAURI_MANIFEST_ASSET.ends_with(".json"));
        assert!(manifest_endpoint().ends_with(TAURI_MANIFEST_ASSET));
    }
}
