//! 按需下载与缓存（M3 生态域 1 · Task 1 · DR-PH-3）。
//!
//! 布局：`<dataRoot>/plugin-host/<version>/<entry>`。
//! 语义：
//! - 缓存命中且 sha256 一致 → **不联网**（断网可用）；
//! - 未命中 → 下载 → 校验 → 临时文件 + rename（不半写）；
//! - sha256 不符/HTTP 失败 → 报错，不落一个「看起来存在」的文件。
//!
//! TLS：`reqwest` 以 `rustls-no-provider` 编译（与 `crates/platform` 同档位），
//! 这里在使用前安装 ring provider。

use std::fmt;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::manifest::{ManifestError, PluginHostManifest};

/// 下载/校验/缓存的失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheError {
    Manifest(ManifestError),
    Io(String),
    Http(String),
    HttpStatus(u16),
    HashMismatch { expected: String, actual: String },
}

impl fmt::Display for CacheError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manifest(error) => write!(formatter, "{error}"),
            Self::Io(message) => write!(formatter, "{message}"),
            Self::Http(message) => write!(formatter, "下载插件宿主失败：{message}"),
            Self::HttpStatus(status) => write!(formatter, "下载插件宿主返回 HTTP {status}"),
            Self::HashMismatch { expected, actual } => {
                write!(
                    formatter,
                    "插件宿主 sha256 不符：期望 {expected}，实际 {actual}"
                )
            }
        }
    }
}

impl std::error::Error for CacheError {}

impl From<ManifestError> for CacheError {
    fn from(error: ManifestError) -> Self {
        Self::Manifest(error)
    }
}

pub struct AssetCache {
    data_root: PathBuf,
}

impl AssetCache {
    pub fn new(data_root: impl Into<PathBuf>) -> Self {
        Self {
            data_root: data_root.into(),
        }
    }

    /// 缓存的版本目录（`<dataRoot>/plugin-host/<version>`）。
    pub fn cache_dir(&self, manifest: &PluginHostManifest) -> PathBuf {
        self.data_root.join("plugin-host").join(&manifest.version)
    }

    /// 确保资产在本地可用，返回 bundle 的路径。
    pub fn ensure(&self, manifest: &PluginHostManifest) -> Result<PathBuf, CacheError> {
        manifest.validate()?;
        let directory = self.cache_dir(manifest);
        let path = directory.join(&manifest.entry);

        if path.is_file() {
            let bytes = fs::read(&path).map_err(|error| {
                CacheError::Io(format!("读取缓存 {} 失败：{error}", path.display()))
            })?;
            if sha256_hex(&bytes) == manifest.sha256 {
                return Ok(path);
            }
        }

        fs::create_dir_all(&directory).map_err(|error| {
            CacheError::Io(format!(
                "创建缓存目录 {} 失败：{error}",
                directory.display()
            ))
        })?;
        install_tls_provider();
        let response = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|error| CacheError::Http(error.to_string()))?
            .get(&manifest.asset_url)
            .send()
            .map_err(|error| CacheError::Http(error.to_string()))?;
        if !response.status().is_success() {
            return Err(CacheError::HttpStatus(response.status().as_u16()));
        }
        let bytes = response
            .bytes()
            .map_err(|error| CacheError::Http(error.to_string()))?
            .to_vec();
        let actual = sha256_hex(&bytes);
        if actual != manifest.sha256 {
            return Err(CacheError::HashMismatch {
                expected: manifest.sha256.clone(),
                actual,
            });
        }

        let temporary = directory.join(format!(".{}.tmp-{}", manifest.entry, std::process::id()));
        fs::write(&temporary, &bytes).map_err(|error| {
            CacheError::Io(format!("写临时文件 {} 失败：{error}", temporary.display()))
        })?;
        // Windows 上 rename 不能覆盖已存在的文件；旧缓存 hash 不符时先删掉。
        fs::remove_file(&path).ok();
        fs::rename(&temporary, &path)
            .map_err(|error| CacheError::Io(format!("落盘 {} 失败：{error}", path.display())))?;
        Ok(path)
    }
}

/// 安装 rustls 的 ring provider（已安装时返回的 Err 可忽略）。
fn install_tls_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("{digest:x}")
}
