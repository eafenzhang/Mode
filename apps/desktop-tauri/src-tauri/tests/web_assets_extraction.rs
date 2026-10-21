//! 内嵌前端解压的回归测试。
//!
//! tauri 默认启用 `compression`：`Assets::iter()` 交出的是构建期 Brotli 压缩字节，
//! 只有 `Assets::get()` 会解压。LAN 静态服务直接读这些文件，写错一边就会把压缩字节
//! 当成 HTML/JS 发出去。这里用 mock app 的假资源把「必须走 get()」钉住。

use std::borrow::Cow;
use std::fs;

use mode_tauri::web_assets::extract_to;
use tauri::utils::assets::{AssetKey, AssetsIter, CspHash};
use tauri::{Assets, Runtime};

/// 模拟 tauri 的 compression 行为：`iter()` 给压缩字节，`get()` 给解压后的字节。
struct CompressedAssets {
    plain: &'static [(&'static str, &'static [u8])],
}

impl<R: Runtime> Assets<R> for CompressedAssets {
    fn get(&self, key: &AssetKey) -> Option<Cow<'_, [u8]>> {
        self.plain
            .iter()
            .find(|(path, _)| *path == key.as_ref())
            .map(|(_, bytes)| Cow::Borrowed(*bytes))
    }

    fn iter(&self) -> Box<AssetsIter<'_>> {
        Box::new(
            self.plain
                .iter()
                .map(|(path, _)| (Cow::Borrowed(*path), Cow::Borrowed(&b"<brotli>"[..]))),
        )
    }

    fn csp_hashes(&self, _html_path: &AssetKey) -> Box<dyn Iterator<Item = CspHash<'_>> + '_> {
        Box::new(std::iter::empty())
    }
}

#[test]
fn extraction_writes_decompressed_bytes_and_skips_desktop_entries() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(CompressedAssets {
            plain: &[
                ("/web.html", b"<html>web</html>"),
                ("/index.html", b"<html>desktop</html>"),
                ("/about.html", b"<html>about</html>"),
                ("/assets/app.js", b"console.log('plain')"),
            ],
        }))
        .expect("mock app");
    let base = std::env::temp_dir().join(format!("mode-web-assets-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);

    let extracted = extract_to(app.handle(), &base).expect("extract");
    // 关键回归：写盘的必须是 `get()` 的解压字节，不能是 `iter()` 的压缩字节。
    assert_eq!(
        fs::read(extracted.join("web.html")).unwrap(),
        b"<html>web</html>"
    );
    assert_eq!(
        fs::read(extracted.join("assets/app.js")).unwrap(),
        b"console.log('plain')"
    );
    assert!(!extracted.join("index.html").exists());
    assert!(!extracted.join("about.html").exists());
    assert!(extracted.join(".ready").is_file());
    // `.ready` 命中后第二次调用直接复用，不重新解压。
    assert_eq!(
        extract_to(app.handle(), &base).as_deref(),
        Some(extracted.as_path())
    );

    let _ = fs::remove_dir_all(&base);
}
