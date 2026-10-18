//! 插件宿主资产的下载/校验/缓存（M3 生态域 1 · Task 1）。
//!
//! 用本地 HTTP 服务模拟 CDN：不依赖网络，命中缓存时断言**不再发请求**。

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use plugin_host::{AssetCache, CacheError, ManifestError, PluginHostManifest};

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn temp_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mode-plugin-host-{tag}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

/// 只接受一次请求的本地 HTTP 服务，返回 URL 与命中计数。
fn serve_once(body: Vec<u8>) -> (String, Arc<AtomicUsize>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("绑定端口");
    let address = listener.local_addr().expect("取地址");
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    let handle = thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            counter.fetch_add(1, Ordering::SeqCst);
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request);
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).expect("写响应头");
            stream.write_all(&body).expect("写响应体");
        }
    });
    (
        format!("http://127.0.0.1:{}/plugin-host.mjs", address.port()),
        hits,
        handle,
    )
}

fn manifest(asset_url: &str, body: &[u8]) -> PluginHostManifest {
    PluginHostManifest {
        schema_version: 1,
        version: "0.1.0".to_string(),
        platform: "win32-x64".to_string(),
        entry: "plugin-host.mjs".to_string(),
        asset_url: asset_url.to_string(),
        sha256: sha256_hex(body),
    }
}

#[test]
fn downloads_verifies_and_then_uses_the_cache() {
    let body = b"console.log('plugin host')\n".to_vec();
    let (url, hits, server) = serve_once(body.clone());
    let root = temp_root("download");
    let cache = AssetCache::new(&root);
    let manifest = manifest(&url, &body);

    let path = cache.ensure(&manifest).expect("首次下载");
    assert_eq!(fs::read(&path).expect("读缓存"), body);
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    let again = cache.ensure(&manifest).expect("缓存命中");
    assert_eq!(again, path);
    assert_eq!(hits.load(Ordering::SeqCst), 1, "命中缓存不应再联网");
    server.join().expect("服务线程");
}

#[test]
fn tampered_hash_is_rejected_and_leaves_no_file() {
    let body = b"console.log('tampered')\n".to_vec();
    let (url, _hits, server) = serve_once(body.clone());
    let root = temp_root("tampered");
    let cache = AssetCache::new(&root);
    let mut manifest = manifest(&url, &body);
    manifest.sha256 = "0".repeat(64);

    let error = cache.ensure(&manifest).expect_err("sha256 不符必须失败");
    assert!(matches!(error, CacheError::HashMismatch { .. }));
    let path = cache.cache_dir(&manifest).join(&manifest.entry);
    assert!(!path.exists(), "校验失败不应落盘");
    server.join().expect("服务线程");
}

#[test]
fn offline_with_valid_cache_returns_the_cached_file() {
    let body = b"console.log('offline')\n".to_vec();
    let (url, hits, server) = serve_once(body.clone());
    let root = temp_root("offline");
    let cache = AssetCache::new(&root);
    let mut manifest = manifest(&url, &body);
    let path = cache.ensure(&manifest).expect("首次下载");

    // 断网：URL 指向一个不会有人监听的端口；缓存命中时不应尝试联网。
    manifest.asset_url = "http://127.0.0.1:9/never".to_string();
    let cached = cache.ensure(&manifest).expect("缓存应可用");
    assert_eq!(cached, path);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    server.join().expect("服务线程");
}

#[test]
fn entry_must_be_a_single_file_name() {
    let root = temp_root("entry");
    let cache = AssetCache::new(&root);
    let mut manifest = manifest("http://127.0.0.1:9/never", b"x");
    manifest.entry = "../evil.mjs".to_string();
    let error = cache.ensure(&manifest).expect_err("路径逃逸必须失败");
    assert!(matches!(
        error,
        CacheError::Manifest(ManifestError::InvalidEntry(_))
    ));
}

#[test]
fn parse_rejects_invalid_json_and_schema() {
    assert!(matches!(
        PluginHostManifest::parse(b"not json"),
        Err(ManifestError::InvalidJson(_))
    ));
    let wrong_schema = br#"{
        "schemaVersion": 2,
        "version": "0.1.0",
        "platform": "win32-x64",
        "entry": "plugin-host.mjs",
        "assetUrl": "https://example.invalid/x.mjs",
        "sha256": "0000000000000000000000000000000000000000000000000000000000000000"
    }"#;
    assert!(matches!(
        PluginHostManifest::parse(wrong_schema),
        Err(ManifestError::UnsupportedSchema(2))
    ));
}
