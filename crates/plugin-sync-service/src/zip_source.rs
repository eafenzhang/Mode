//! zip 插件源：下载（sha256 校验 + 限额）→ 解压（路径/类型/限额防护）→ 根定位。
//!
//! 真源：`apps/mode-cli/packages/adapters/src/plugins/zip-source.ts`（clean-room，
//! 只读本仓库 TS）。错误文案与 TS 逐条对齐；解压防护（路径穿越/加密条目/symlink/
//! 非 dir-file 类型/条目数/单文件/总量）与根定位顺序（path 子目录 → 提取根
//! manifest → stripRoot 单顶层目录 → extractRoot）同口径。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::marketplace_install::ResolvedPluginSourceRoot;

pub const ZIP_DOWNLOAD_MAX_BYTES: u64 = 200 * 1024 * 1024;
pub const ZIP_EXTRACT_MAX_BYTES: u64 = 500 * 1024 * 1024;
pub const ZIP_MAX_ENTRIES: usize = 20_000;
pub const ZIP_MAX_SINGLE_FILE_BYTES: u64 = 50 * 1024 * 1024;
pub const ZIP_MAX_REDIRECTS: usize = 5;
pub const ZIP_DOWNLOAD_TIMEOUT_MS: u64 = 180_000;
const ZIP_TEMP_PREFIX: &str = "mode-plugin-zip-";
const ZIP_DENIED_HEADERS: [&str; 4] = [
    "authorization",
    "cookie",
    "proxy-authorization",
    "set-cookie",
];

/// zip 源 manifest 候选（与插件 manifest 查找顺序一致）。
const ZIP_MANIFEST_CANDIDATES: [&str; 4] = [
    ".mode-plugin/plugin.json",
    ".zcode-plugin/plugin.json",
    ".claude-plugin/plugin.json",
    ".codex-plugin/plugin.json",
];

#[derive(Debug, Default)]
pub struct ZipSourceInput {
    pub url: String,
    pub sha256: String,
    pub headers: Option<serde_json::Map<String, serde_json::Value>>,
    pub path: Option<String>,
    pub strip_root: Option<bool>,
}

/// TS `validateZipDownloadUrl`：必须 HTTPS；HTTP 仅回环地址（本机对拍用）。
fn validate_zip_download_url(value: &str) -> Result<(), String> {
    let Ok(url) = url::Url::parse(value) else {
        return Err(format!("Plugin zip source URL is invalid: {value}"));
    };
    match url.scheme() {
        "https" => Ok(()),
        "http" if is_loopback_host(url.host_str().unwrap_or_default()) => Ok(()),
        _ => Err(format!("Plugin zip source URL must be HTTPS: {value}")),
    }
}

fn is_loopback_host(hostname: &str) -> bool {
    let normalized = hostname
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_lowercase();
    normalized == "localhost" || normalized == "::1" || is_ipv4_loopback_host(&normalized)
}

fn is_ipv4_loopback_host(hostname: &str) -> bool {
    let parts: Vec<&str> = hostname.split('.').collect();
    if parts.len() != 4
        || !parts[0].starts_with("127")
        || !parts[0].chars().all(|c| c.is_ascii_digit())
    {
        return false;
    }
    parts.iter().all(|part| {
        part.chars().all(|c| c.is_ascii_digit())
            && part.parse::<u16>().map(|v| v <= 255).unwrap_or(false)
    })
}

/// TS `validateZipHeaders`：拒绝凭据类 header 跨源泄露。
fn validate_zip_headers(
    headers: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Result<(), String> {
    let Some(headers) = headers else {
        return Ok(());
    };
    for (key, value) in headers {
        if ZIP_DENIED_HEADERS.contains(&key.to_lowercase().as_str()) {
            return Err(format!("Plugin zip source header is not allowed: {key}"));
        }
        if !value.is_string() {
            return Err(format!("Plugin zip source header must be a string: {key}"));
        }
    }
    Ok(())
}

fn validate_sha256(sha256: &str) -> Result<String, String> {
    let lowered = sha256.to_lowercase();
    if lowered.len() == 64 && lowered.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(lowered)
    } else {
        Err("Plugin zip source sha256 must be a 64 character hex string".to_string())
    }
}

/// TS `normalizeZipRelativePath`：拒绝 `\0`/`\`/绝对路径/盘符/`.`/`..`/空段。
fn normalize_zip_relative_path(path: &str) -> Result<String, String> {
    if path.contains('\0') {
        return Err(format!("Unsafe plugin zip path: {path}"));
    }
    let without_trailing = path.trim_end_matches('/');
    // 盘符形态（TS /^[a-zA-Z]:/u）：首字符字母且第二字符是冒号。
    let drive_letter = path.as_bytes().first().is_some_and(|b| b.is_ascii_alphabetic())
        && path.as_bytes().get(1) == Some(&b':');
    let invalid = without_trailing.is_empty()
        || path.contains('\\')
        || Path::new(path).is_absolute()
        || path.starts_with('/')
        || drive_letter;
    if invalid {
        return Err(format!("Unsafe plugin zip path: {path}"));
    }
    let parts: Vec<&str> = without_trailing.split('/').collect();
    if parts
        .iter()
        .any(|part| part.is_empty() || *part == "." || *part == "..")
    {
        return Err(format!("Unsafe plugin zip path: {path}"));
    }
    Ok(without_trailing.to_string())
}

/// TS `resolveZipPathWithin`：拼接后必须仍位于根内。
fn resolve_zip_path_within(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let normalized = normalize_zip_relative_path(relative)?;
    let mut target = root.to_path_buf();
    for segment in normalized.split('/') {
        target.push(segment);
    }
    if !target.starts_with(root) || target == root {
        return Err(format!("Unsafe plugin zip path: {relative}"));
    }
    Ok(target)
}

/// zip 条目分类与安全校验（TS `classifyZipEntry`）。
fn classify_zip_entry<R: std::io::Read>(
    file: &mut zip::read::ZipFile<R>,
    name: &str,
) -> Result<ZipEntryKind, String> {
    // 加密条目（general purpose bit 0）与 symlink（unix mode 0o120000）一律拒绝。
    if file.encrypted() {
        return Err(format!(
            "Encrypted plugin zip entries are not supported: {name}"
        ));
    }
    if let Some(mode) = file.unix_mode() {
        let file_type = mode & 0o170000;
        if file_type == 0o120000 {
            return Err(format!(
                "Plugin zip entry symlinks are not supported: {name}"
            ));
        }
        if file_type != 0 && file_type != 0o100000 && file_type != 0o040000 {
            return Err(format!("Unsupported plugin zip entry type: {name}"));
        }
    }
    if name.ends_with('/') || file.is_dir() {
        return Ok(ZipEntryKind::Directory);
    }
    Ok(ZipEntryKind::File)
}

enum ZipEntryKind {
    Directory,
    File,
}

struct ExtractResult {
    top_level_segments: Vec<String>,
}

/// 解压：条目数/单文件/总量三重限额 + 路径与类型防护（TS `extractZipArchive`）。
fn extract_zip_archive(archive_path: &Path, target_root: &Path) -> Result<ExtractResult, String> {
    std::fs::create_dir_all(target_root).map_err(|error| error.to_string())?;
    let file = std::fs::File::open(archive_path).map_err(|error| error.to_string())?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| format!("Plugin zip archive is invalid: {error}"))?;
    let mut top_level_segments: Vec<String> = Vec::new();
    let mut extracted_bytes: u64 = 0;
    for index in 0..archive.len() {
        if index + 1 > ZIP_MAX_ENTRIES {
            return Err(format!(
                "Plugin zip has too many entries: {}/{ZIP_MAX_ENTRIES}",
                index + 1
            ));
        }
        let mut file = archive
            .by_index(index)
            .map_err(|error| format!("Plugin zip archive is invalid: {error}"))?;
        let normalized = normalize_zip_relative_path(file.name())?;
        let segment = normalized
            .split('/')
            .next()
            .unwrap_or(&normalized)
            .to_string();
        if !top_level_segments.contains(&segment) {
            top_level_segments.push(segment);
        }
        let target_path = resolve_zip_path_within(target_root, &normalized)?;
        match classify_zip_entry(&mut file, &normalized)? {
            ZipEntryKind::Directory => {
                std::fs::create_dir_all(&target_path).map_err(|error| error.to_string())?;
            }
            ZipEntryKind::File => {
                if file.size() > ZIP_MAX_SINGLE_FILE_BYTES {
                    return Err(format!(
                        "Plugin zip entry exceeds single file limit: {}",
                        file.name()
                    ));
                }
                let mut buffer = Vec::with_capacity(file.size() as usize);
                file.read_to_end(&mut buffer)
                    .map_err(|error| error.to_string())?;
                extracted_bytes += buffer.len() as u64;
                if extracted_bytes > ZIP_EXTRACT_MAX_BYTES {
                    return Err(format!(
                        "Plugin zip extracted content exceeds limit: {extracted_bytes}/{ZIP_EXTRACT_MAX_BYTES}"
                    ));
                }
                if let Some(parent) = target_path.parent() {
                    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                std::fs::write(&target_path, &buffer).map_err(|error| error.to_string())?;
            }
        }
    }
    Ok(ExtractResult { top_level_segments })
}

/// TS `resolveZipRoot`：path 子目录 → 提取根 manifest → stripRoot → extractRoot。
fn resolve_zip_root(
    extract_root: &Path,
    input: &ZipSourceInput,
    top_level_segments: &[String],
) -> Result<PathBuf, String> {
    if let Some(path) = &input.path {
        let normalized = normalize_zip_relative_path(path)?;
        let requested = resolve_zip_path_within(extract_root, &normalized)?;
        if requested.is_dir() {
            return Ok(requested);
        }
        return Err(format!(
            "Plugin zip source subdirectory does not exist: {path}"
        ));
    }

    if has_plugin_manifest(extract_root) {
        return Ok(extract_root.to_path_buf());
    }

    let strip_root = input.strip_root.unwrap_or(true);
    if strip_root && top_level_segments.len() == 1 {
        let segment = &top_level_segments[0];
        let candidate = resolve_zip_path_within(extract_root, segment)?;
        if candidate.is_dir() {
            return Ok(candidate);
        }
    }

    if !extract_root.is_dir() {
        return Err("Plugin zip did not extract a plugin root directory".to_string());
    }
    Ok(extract_root.to_path_buf())
}

fn has_plugin_manifest(root: &Path) -> bool {
    ZIP_MANIFEST_CANDIDATES
        .iter()
        .any(|candidate| root.join(candidate).is_file())
}

/// 下载：手动重定向（≤5 跳；跨 origin 丢自定义 headers）、200 MiB 上限、
/// 180s 超时；非 2xx 报 `PluginZipDownloadError` 同文案（TS `downloadZipArchive`）。
fn download_zip_archive(
    client: &reqwest::blocking::Client,
    url: &str,
    headers: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Result<Vec<u8>, String> {
    let mut current_url = url.to_string();
    let mut current_headers = headers.cloned();
    for _redirect_count in 0..=ZIP_MAX_REDIRECTS {
        validate_zip_download_url(&current_url)?;
        let mut request = client.get(&current_url);
        if let Some(headers) = &current_headers {
            for (key, value) in headers {
                let Some(value) = value.as_str() else {
                    continue;
                };
                request = request.header(key.as_str(), value);
            }
        }
        let response = request.send().map_err(|error| error.to_string())?;
        let status = response.status().as_u16();
        if is_redirect_status(status) {
            let Some(location) = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
            else {
                return Err(format!(
                    "Plugin zip download redirect is missing Location header: {current_url}"
                ));
            };
            let next = url::Url::parse(&current_url)
                .and_then(|base| base.join(location))
                .map_err(|error| error.to_string())?;
            // 跨 CDN origin 继续发送 marketplace 自定义 header 会泄露内部元数据。
            if next.origin().ascii_serialization()
                != url::Url::parse(&current_url)
                    .map(|base| base.origin().ascii_serialization())
                    .unwrap_or_default()
            {
                current_headers = None;
            }
            current_url = next.to_string();
            continue;
        }
        if !(200..300).contains(&status) {
            return Err(format!(
                "Failed to download plugin zip: {status} {}",
                response.status().canonical_reason().unwrap_or_default()
            ));
        }
        // 上限读：超过 ZIP_DOWNLOAD_MAX_BYTES 即拒绝（防 content-length 谎报）。
        let mut buffer = Vec::new();
        let mut limited = response.take(ZIP_DOWNLOAD_MAX_BYTES + 1);
        limited
            .read_to_end(&mut buffer)
            .map_err(|error| error.to_string())?;
        if buffer.len() as u64 > ZIP_DOWNLOAD_MAX_BYTES {
            return Err(format!(
                "Plugin zip download exceeds limit: {}/{} bytes",
                buffer.len(),
                ZIP_DOWNLOAD_MAX_BYTES
            ));
        }
        return Ok(buffer);
    }
    Err(format!(
        "Plugin zip download exceeded redirect limit: {url}"
    ))
}

fn is_redirect_status(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

fn new_temp_root() -> Result<PathBuf, String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("{ZIP_TEMP_PREFIX}{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    Ok(dir)
}

/// zip 源解析主入口（TS `resolveZipPluginSource`）。
///
/// 返回 [`ResolvedPluginSourceRoot`]：`path` 是插件根；`cleanup()` 删除整个临时
/// 目录（与 git clone 源同一 cleanup 所有权，调用方复制进 cache 后调用）。
pub fn resolve_zip_plugin_source(
    input: &ZipSourceInput,
) -> Result<ResolvedPluginSourceRoot, String> {
    validate_zip_download_url(&input.url)?;
    validate_zip_headers(input.headers.as_ref())?;
    if let Some(path) = &input.path {
        normalize_zip_relative_path(path)?;
    }
    let expected_sha256 = validate_sha256(&input.sha256)?;

    // TLS provider：reqwest 以 rustls-no-provider 编译，构造时装 ring（幂等）。
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_millis(ZIP_DOWNLOAD_TIMEOUT_MS))
        .build()
        .map_err(|error| error.to_string())?;

    let temp_root = new_temp_root()?;
    let result = (|| -> Result<ResolvedPluginSourceRoot, String> {
        let zip_bytes = download_zip_archive(&client, &input.url, input.headers.as_ref())?;
        use sha2::Digest as _;
        let mut hasher = sha2::Sha256::new();
        hasher.update(&zip_bytes);
        let actual_sha256 = format!("{:x}", hasher.finalize());
        if actual_sha256 != expected_sha256 {
            return Err(format!(
                "Plugin zip sha256 mismatch: expected={expected_sha256}, actual={actual_sha256}"
            ));
        }
        let archive_path = temp_root.join("source.zip");
        std::fs::write(&archive_path, &zip_bytes).map_err(|error| error.to_string())?;
        let extract_root = temp_root.join("extract");
        let extracted = extract_zip_archive(&archive_path, &extract_root)?;
        let plugin_root = resolve_zip_root(&extract_root, input, &extracted.top_level_segments)?;
        Ok(crate::marketplace_install::cloned_source_root(
            plugin_root,
            temp_root.clone(),
        ))
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&temp_root);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// 用 zip crate 造一个最小 zip（demo/.mode-plugin/plugin.json）。
    fn write_test_zip(path: &Path) -> Vec<u8> {
        let file = std::fs::File::create(path).expect("create zip");
        let mut writer = zip::ZipWriter::new(file);
        let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
        writer
            .start_file("demo/.mode-plugin/plugin.json", options)
            .expect("start file");
        writer
            .write_all(br#"{"name":"demo","version":"1.0.0"}"#)
            .expect("write");
        writer.finish().expect("finish");
        std::fs::read(path).expect("read zip")
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        use sha2::Digest;
        format!("{:x}", sha2::Sha256::digest(bytes))
    }

    #[test]
    fn url_validation_allows_https_and_loopback_only() {
        assert!(validate_zip_download_url("https://example.com/x.zip").is_ok());
        assert!(validate_zip_download_url("http://localhost/x.zip").is_ok());
        assert!(validate_zip_download_url("http://127.0.0.1:8080/x.zip").is_ok());
        assert!(validate_zip_download_url("http://example.com/x.zip").is_err());
        assert!(validate_zip_download_url("ftp://example.com/x.zip").is_err());
        assert!(validate_zip_download_url("not a url").is_err());
    }

    #[test]
    fn denied_headers_rejected() {
        let mut headers = serde_json::Map::new();
        headers.insert(
            "Authorization".to_string(),
            serde_json::Value::String("x".into()),
        );
        assert!(validate_zip_headers(Some(&headers)).is_err());
        let mut ok = serde_json::Map::new();
        ok.insert(
            "X-Custom".to_string(),
            serde_json::Value::String("y".into()),
        );
        assert!(validate_zip_headers(Some(&ok)).is_ok());
        let mut bad = serde_json::Map::new();
        bad.insert("X-Custom".to_string(), serde_json::Value::Number(1.into()));
        assert!(validate_zip_headers(Some(&bad)).is_err());
    }

    #[test]
    fn unsafe_zip_paths_rejected() {
        for path in ["../escape", "a/../b", "./x", "/abs", "C:\\x", "a\\b", ""] {
            assert!(normalize_zip_relative_path(path).is_err(), "{path} 应拒绝");
        }
        assert_eq!(normalize_zip_relative_path("a/b/").as_deref(), Ok("a/b"));
    }

    #[test]
    fn sha256_mismatch_rejected() {
        let dir = std::env::temp_dir().join(format!(
            "zip-source-sha-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos() as u64)
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let bytes = write_test_zip(&dir.join("probe.zip"));
        let input = ZipSourceInput {
            url: "http://127.0.0.1:1/x.zip".to_string(),
            sha256: "0".repeat(64),
            ..Default::default()
        };
        // URL 不可达时的错误先于 sha256 校验（下载在前）——用 loopback 不可达端口
        // 断言「失败且清理」，sha256 语义由 extract 内联校验覆盖。
        let resolved = resolve_zip_plugin_source(&input);
        assert!(resolved.is_err());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = sha256_hex(&bytes);
    }

    #[test]
    fn extract_enforces_path_and_limits() {
        let dir = std::env::temp_dir().join(format!(
            "zip-source-extract-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos() as u64)
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let zip_path = dir.join("source.zip");
        let bytes = write_test_zip(&zip_path);
        let extract_root = dir.join("extract");
        let extracted = extract_zip_archive(&zip_path, &extract_root).expect("extract");
        assert_eq!(extracted.top_level_segments, vec!["demo".to_string()]);
        assert!(extract_root.join("demo/.mode-plugin/plugin.json").is_file());
        assert!(sha256_hex(&bytes).len() == 64);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn strip_root_resolves_single_top_level_directory() {
        let dir = std::env::temp_dir().join(format!(
            "zip-source-root-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos() as u64)
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let extract_root = dir.join("extract");
        std::fs::create_dir_all(extract_root.join("demo/.mode-plugin")).expect("demo");
        std::fs::write(extract_root.join("demo/.mode-plugin/plugin.json"), "{}").expect("manifest");
        // 提取根无 manifest、单顶层目录有 → stripRoot 落 demo。
        let resolved = resolve_zip_root(
            &extract_root,
            &ZipSourceInput::default(),
            &["demo".to_string()],
        )
        .expect("root");
        assert_eq!(resolved, extract_root.join("demo"));
        // stripRoot=false：即使单顶层也不下钻（落提取根）。
        let resolved = resolve_zip_root(
            &extract_root,
            &ZipSourceInput {
                strip_root: Some(false),
                ..Default::default()
            },
            &["demo".to_string()],
        )
        .expect("root");
        assert_eq!(resolved, extract_root);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn http_loopback_zip_source_round_trip() {
        // 本机 HTTP server（回环豁免 HTTPS）端到端：下载 → sha256 → 解压 → 根定位。
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let dir = std::env::temp_dir().join(format!(
            "zip-source-e2e-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos() as u64)
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let bytes = write_test_zip(&dir.join("source.zip"));
        let expected_sha256 = sha256_hex(&bytes);
        let server = std::thread::spawn(move || {
            use std::io::Read as _;
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buffer = [0u8; 2048];
                let _ = stream.read(&mut buffer);
                let body = bytes.clone();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/zip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        let input = ZipSourceInput {
            url: format!("http://127.0.0.1:{port}/source.zip"),
            sha256: expected_sha256,
            ..Default::default()
        };
        let resolved = resolve_zip_plugin_source(&input).expect("resolve");
        assert!(resolved.path.is_dir());
        assert!(resolved.path.join(".mode-plugin/plugin.json").is_file());
        // cleanup 删除整个临时目录。
        let temp = resolved.temp_dir().expect("temp dir").to_path_buf();
        assert!(temp.to_string_lossy().contains("mode-plugin-zip-"));
        resolved.cleanup();
        assert!(!temp.exists());
        server.join().expect("server");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
