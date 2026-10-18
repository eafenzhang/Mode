//! 插件宿主资产清单（M3 生态域 1 · Task 1）。
//!
//! 这是**新资产**的清单，不是旧实现的格式：字段由本计划定义，CDN 上传时按此生成。
//! `deny_unknown_fields` 是刻意的——清单加字段必须升 `schemaVersion`，避免 CDN 与客户端
//! 对同一份 JSON 的理解悄悄漂移。

use serde::{Deserialize, Serialize};

/// 当前唯一支持的清单版本。
pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostManifest {
    pub schema_version: u32,
    pub version: String,
    pub platform: String,
    /// 缓存里的文件名；必须是单层文件名（不允许路径分隔符或 `..`）。
    pub entry: String,
    pub asset_url: String,
    /// bundle 的 sha256（64 位十六进制）。
    pub sha256: String,
}

impl PluginHostManifest {
    /// 从 CDN 的 JSON 文本解析并校验。
    pub fn parse(bytes: &[u8]) -> Result<Self, ManifestError> {
        let manifest: Self = serde_json::from_slice(bytes)
            .map_err(|error| ManifestError::InvalidJson(error.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.schema_version != MANIFEST_SCHEMA_VERSION {
            return Err(ManifestError::UnsupportedSchema(self.schema_version));
        }
        for (field, value) in [
            ("version", &self.version),
            ("platform", &self.platform),
            ("entry", &self.entry),
            ("assetUrl", &self.asset_url),
            ("sha256", &self.sha256),
        ] {
            if value.trim().is_empty() {
                return Err(ManifestError::EmptyField(field));
            }
        }
        if self.entry.contains(['/', '\\']) || self.entry.contains("..") {
            return Err(ManifestError::InvalidEntry(self.entry.clone()));
        }
        if self.sha256.len() != 64 || !self.sha256.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return Err(ManifestError::InvalidSha256(self.sha256.clone()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    InvalidJson(String),
    UnsupportedSchema(u32),
    EmptyField(&'static str),
    InvalidEntry(String),
    InvalidSha256(String),
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidJson(message) => write!(formatter, "清单不是合法 JSON：{message}"),
            Self::UnsupportedSchema(version) => {
                write!(formatter, "不支持的 schemaVersion：{version}")
            }
            Self::EmptyField(field) => write!(formatter, "清单字段 {field} 为空"),
            Self::InvalidEntry(entry) => write!(formatter, "entry 必须是单层文件名：{entry}"),
            Self::InvalidSha256(value) => write!(formatter, "sha256 形状非法：{value}"),
        }
    }
}

impl std::error::Error for ManifestError {}
