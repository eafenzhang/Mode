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
/// ## 配置必须写在 `tauri.conf.json`，且 `pubkey` 不能省（**实测过的坑**）
///
/// 插件通过 `tauri_plugin_updater::Config` 反序列化 `plugins.updater` 段。该结构体的
/// `pubkey: String` **没有 `#[serde(default)]`**，于是：
///
/// - 整段缺席 → Tauri 传 `null` → `PluginInitialization("updater", "invalid type: null,
///   expected struct Config")`，**app 启动即 panic**；
/// - 段存在但少了 `pubkey` 键 → `missing field pubkey`，同样起不来。
///
/// 所以「M1 不设 pubkey」这件事**做不到字面意思**：字段必须在。M1 的取值是**空串**，
/// 语义上等价于「没有密钥」——空串走到 `verify_signature` 里会在
/// `base64_to_string("")` → `PublicKey::decode("")` 处报错，**fail closed**，
/// 不会静默放行未签名的更新（已核对 `tauri-plugin-updater-2.14.0/src/updater.rs:1901`）。
/// M5 签发真密钥时把这个值换掉即可。
///
/// 端点同时写在 `crates/updater` 的 [`manifest_endpoint`] 与 conf 两处，原因见
/// `the_app_conf_endpoint_matches_the_constant`：`Builder` 没有 `endpoints()`，
/// 端点只能从配置来，常量是给代码与单测看的那份；两者由测试钉死，改一处即红。
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

    /// app 的 `tauri.conf.json`。`include_str!` 让路径在**编译期**校验：conf 被挪走时
    /// 这里直接编译失败，不会留一条运行时才炸的路径。
    ///
    /// 本 crate 反过来知道 app 的 conf 位置，是刻意的：插件的运行期配置只能来自 conf，
    /// 而端点常量住在本 crate，两者必须一致。让**常量这一侧**来断言，是为了让
    /// 「端点是什么」这件事在代码里有唯一可读的出处，conf 只是它的落地副本。
    const APP_CONF: &str = include_str!("../../../apps/desktop-tauri/src-tauri/tauri.conf.json");

    fn app_conf() -> serde_json::Value {
        serde_json::from_str(APP_CONF).expect("tauri.conf.json 必须是合法 JSON")
    }

    /// `Builder` 没有 `endpoints()`，端点只能写在 conf 里；本 crate 的
    /// [`manifest_endpoint`] 是同一串 URL 的另一份。这条断言把两份钉死，改一处即红。
    #[test]
    fn the_app_conf_endpoint_matches_the_constant() {
        let conf = app_conf();
        let endpoints = conf["plugins"]["updater"]["endpoints"]
            .as_array()
            .expect("plugins.updater.endpoints 必须存在且是数组");
        assert_eq!(endpoints.len(), 1, "M1 只声明一个端点");
        assert_eq!(
            endpoints[0].as_str(),
            Some(manifest_endpoint().as_str()),
            "conf 的端点与 manifest_endpoint() 漂移了"
        );
    }

    /// `pubkey` 是**必填字段**，它的缺席正是 app 起不来的原因（见 [`plugin`] 的文档）。
    /// 这条断言把「M1 的取值是空串」这个刻意的状态写下来：M5 签发真密钥时该断言会红，
    /// 那时要连带把上面 `plugin` 的文档与 spec §5 一起改——这是有意的摩擦。
    #[test]
    fn the_app_conf_declares_an_empty_pubkey_for_m1() {
        let conf = app_conf();
        let pubkey = conf["plugins"]["updater"]
            .get("pubkey")
            .expect("plugins.updater.pubkey 缺失会让 app 启动即 panic");
        assert_eq!(
            pubkey.as_str(),
            Some(""),
            "M1 无签名密钥，此处应为空串（fail closed）；\
             M5 换成真 key 时请同步更新本断言、plugin 的文档与 spec §5"
        );
    }
}
