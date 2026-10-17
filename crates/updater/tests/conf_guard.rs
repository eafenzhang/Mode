//! conf ↔ 常量的守卫（原 `src/lib.rs` 的 `#[cfg(test)] mod tests`）。
//!
//! 放在 `tests/` 而不是 lib 单测模块里，是为了让
//! `crates/updater/build.rs` 的 `cargo:rustc-link-arg-tests` 真正生效：
//! `rustc-link-arg-tests` 只对集成测试目标生效，lib 单测目标拿不到 Windows 的
//! comctl32 v6 清单，会在启动阶段 `STATUS_ENTRYPOINT_NOT_FOUND`（与
//! `crates/platform/build.rs` 的注释同一个原因）。
//!
//! **约定**：以后加本 crate 的测试请继续放 `tests/`。

use updater::{manifest_endpoint, TAURI_MANIFEST_ASSET};

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

/// `Builder` 没有 `endpoints()`，端点只能写在 conf 里；本 crate 的
/// [`manifest_endpoint`] 是同一串 URL 的另一份。这条断言把两份钉死，改一处即红。
#[test]
fn the_app_conf_endpoint_matches_the_constant() {
    let conf = app_conf();
    let endpoints = conf["plugins"]["updater"]["endpoints"]
        .as_array()
        .expect("plugins.updater.endpoints 必须存在且是数组");
    assert_eq!(endpoints.len(), 1, "只声明一个端点");
    assert_eq!(
        endpoints[0].as_str(),
        Some(manifest_endpoint().as_str()),
        "conf 的端点与 manifest_endpoint() 漂移了"
    );
}

/// `pubkey` 是**必填字段**，它的缺席正是 app 起不来的原因（见 `updater::plugin` 的文档）。
///
/// M1–M4 期间这条断言钉的是「空串 = 没有密钥、fail closed」。M5 Task 1 生成并提交了
/// 真实公钥，语义随之变成「用这把密钥校验签名」：断言也从「等于空串」改成
/// 「非空、且能解码成一把可用的 minisign 公钥」。这是**有意的语义变更**，
/// 不是为了让测试变绿而删断言。
///
/// 空串的 fail-closed 语义仍然成立（`PublicKey::decode` 对空输入报错）；本断言防止
/// 有人把 conf 又改回空串却以为更新还能用。
#[test]
fn the_app_conf_declares_a_real_minisign_pubkey() {
    use base64::Engine as _;

    let conf = app_conf();
    let pubkey = conf["plugins"]["updater"]
        .get("pubkey")
        .expect("plugins.updater.pubkey 缺失会让 app 启动即 panic")
        .as_str()
        .expect("plugins.updater.pubkey 必须是字符串");

    assert!(
        !pubkey.is_empty(),
        "pubkey 为空串时更新链 fail closed（装好的应用无法更新）；\
         M5 起这里必须是真实 minisign 公钥的 base64"
    );

    // 插件在运行期正是这样做的：先 base64 解码成 `.pub` 文件文本，再交给
    // `minisign_verify::PublicKey::decode`（tauri-plugin-updater 2.14.0
    // `src/updater.rs:1901`）。这里用同一套解码验证「非空」之外的可解析性。
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(pubkey)
        .expect("pubkey 必须是 base64（.pub 文件的字节）");
    let decoded = String::from_utf8(decoded).expect("pubkey 解码后必须是 UTF-8 文本");
    assert!(
        decoded.starts_with("untrusted comment: minisign public key"),
        "pubkey 解码后不是 minisign 公钥文件格式：{decoded:?}"
    );
    minisign_verify::PublicKey::decode(&decoded).expect("pubkey 必须能被 minisign-verify 解析");
}
