//! 用**已提交**的 `tauri.conf.json` 公钥校验发布产物的 updater `.sig`（M5 评审 B4）。
//!
//! 为什么需要这个独立程序：CI 的 Windows 发布腿原先只检查 `.sig` 文件**存在**。
//! 于是两种坏情况都能发出全绿 Release：
//! - `plugins.updater.pubkey` 回退成空串（应用装上后永远无法更新）；
//! - CI 私钥与入库 pubkey 不是一对（签名文件在，但客户端永远验不过）。
//!
//! 这里用与 `tauri-plugin-updater` 相同的 `minisign-verify` 做真正的密码学校验：
//! 公钥只从**仓库里的** conf 读取，任何输入缺失、为空、格式错误或验签失败都
//! 以非 0 退出（fail closed），由发布腿的 step 直接挡住 `publish`。
//!
//! 用法（CI 与本地相同）：
//!
//! ```text
//! cargo run -q -p updater-verify -- \
//!   apps/desktop-tauri/src-tauri/tauri.conf.json \
//!   dist/tauri/Mode-<版本>-win-x64.exe \
//!   dist/tauri/Mode-<版本>-win-x64.exe.sig
//! ```
//!
//! 第三参数 `allow_legacy` 与插件一致取 `true`
//! （`tauri-plugin-updater-2.14.0/src/updater.rs:1907`）；发布产物由 Tauri 签出
//! prehashed 签名，两种模式都能通过，这里不为闸门另造一套语义。

use std::path::Path;
use std::process::ExitCode;

use base64::Engine as _;

fn fail(message: impl std::fmt::Display) -> ExitCode {
    eprintln!("[verify-updater-signature] FAIL: {message}");
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [conf_path, artifact_path, signature_path] = args.as_slice() else {
        return fail("usage: verify-updater-signature <tauri.conf.json> <artifact> <signature>");
    };

    // 1. 公钥只认已提交的 conf，不接受环境变量或第二个来源。
    let conf_text = match std::fs::read_to_string(conf_path) {
        Ok(text) => text,
        Err(error) => return fail(format!("cannot read {conf_path}: {error}")),
    };
    let conf: serde_json::Value = match serde_json::from_str(&conf_text) {
        Ok(value) => value,
        Err(error) => return fail(format!("{conf_path} is not valid JSON: {error}")),
    };
    let pubkey_b64 = conf
        .get("plugins")
        .and_then(|plugins| plugins.get("updater"))
        .and_then(|updater| updater.get("pubkey"))
        .and_then(serde_json::Value::as_str);
    let Some(pubkey_b64) = pubkey_b64 else {
        return fail(format!(
            "{conf_path} has no string plugins.updater.pubkey (the app cannot start without it)"
        ));
    };
    if pubkey_b64.trim().is_empty() {
        // 空串在客户端是 fail closed：装好的应用永远无法更新。这里同样拒绝，
        // 防止「构建成功但更新链永久坏掉」的 Release。
        return fail("plugins.updater.pubkey is empty; the updater would fail closed forever");
    }
    let pubkey_text = match base64::engine::general_purpose::STANDARD.decode(pubkey_b64) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(error) => return fail(format!("pubkey is not UTF-8 text: {error}")),
        },
        Err(error) => return fail(format!("pubkey is not valid base64: {error}")),
    };
    let pubkey = match minisign_verify::PublicKey::decode(&pubkey_text) {
        // minisign_verify::Error 的 Display 足够定位（缺注释、算法不支持等）。
        Ok(pubkey) => pubkey,
        Err(error) => return fail(format!("pubkey is not a usable minisign key: {error}")),
    };

    // 2. `.sig` 文件是**整个 minisign 签名文本的 base64**（Tauri CLI 产出，
    //    latest.json 的 signature 字段也是同一串）。插件先 base64 解码再
    //    Signature::decode；这里完全照做。文件缺失/为空/不是 base64/不是四行
    //    minisign 格式都在这里失败。
    if !Path::new(signature_path).is_file() {
        return fail(format!("signature file does not exist: {signature_path}"));
    }
    let signature_b64 = match std::fs::read_to_string(signature_path) {
        Ok(text) => text,
        Err(error) => return fail(format!("cannot read {signature_path}: {error}")),
    };
    let signature_b64 = signature_b64.trim();
    if signature_b64.is_empty() {
        return fail(format!("signature file is empty: {signature_path}"));
    }
    let signature_text = match base64::engine::general_purpose::STANDARD.decode(signature_b64) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(error) => return fail(format!("signature is not UTF-8 text: {error}")),
        },
        Err(error) => return fail(format!("signature is not valid base64: {error}")),
    };
    let signature = match minisign_verify::Signature::decode(&signature_text) {
        Ok(signature) => signature,
        Err(error) => return fail(format!("cannot decode {signature_path}: {error}")),
    };

    // 3. 安装包必须存在且非空：空文件配合空签名不应被当作「验过」。
    let data = match std::fs::read(artifact_path) {
        Ok(data) => data,
        Err(error) => return fail(format!("cannot read {artifact_path}: {error}")),
    };
    if data.is_empty() {
        return fail(format!("artifact is empty: {artifact_path}"));
    }

    // 4. 真正的密码学校验：公钥、密钥 id、产物字节（或 BLAKE2b 预哈希）与
    //    trusted comment 的全局签名都必须匹配。
    if let Err(error) = pubkey.verify(&data, &signature, true) {
        return fail(format!(
            "signature does not match the committed pubkey or the artifact: {error}"
        ));
    }

    println!(
        "[verify-updater-signature] OK: {} bytes verified against the committed pubkey ({})",
        data.len(),
        signature.trusted_comment()
    );
    ExitCode::SUCCESS
}
