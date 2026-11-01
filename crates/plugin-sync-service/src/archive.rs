//! ustar 归档写读 + gzip（`pluginSyncArchive.ts` 的逐字节对齐实现）。
//!
//! 支持普通文件、目录与内存条目；导出遇到软链等非普通文件报错，导入遇到未知
//! typeflag 报错。header 字段（mode/uid/gid/size/mtime/checksum/ustar/mode）
//! 与 TS 保持一致。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};

use crate::paths::{normalize_relative_path, resolve_within};

const TAR_BLOCK_SIZE: usize = 512;
const TAR_END_BLOCK_BYTES: usize = TAR_BLOCK_SIZE * 2;

pub const PLUGIN_SYNC_METADATA_ARCHIVE_PATH: &str = ".mode-plugin-sync.json";

pub enum ArchiveEntry {
    Source {
        source_path: PathBuf,
        archive_path: String,
    },
    Content {
        archive_path: String,
        content: Vec<u8>,
        mode: Option<u32>,
        mtime_ms: Option<u64>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveMetadataPlugin {
    pub name: String,
    pub plugin_id: String,
    pub directory_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveMetadata {
    pub plugins: Vec<ArchiveMetadataPlugin>,
}

pub fn create_archive(
    entries: &[ArchiveEntry],
    metadata: &ArchiveMetadata,
) -> Result<Vec<u8>, String> {
    let value = serde_json::to_value(metadata).map_err(|error| error.to_string())?;
    create_archive_with_metadata_value(entries, value)
}

/// 与 `create_archive` 相同，但 metadata 是任意 JSON（marketplaceSources 等扩展字段）。
pub(crate) fn create_archive_with_metadata_value(
    entries: &[ArchiveEntry],
    metadata: serde_json::Value,
) -> Result<Vec<u8>, String> {
    let mut parts: Vec<u8> = Vec::new();
    for entry in entries {
        match entry {
            ArchiveEntry::Source {
                source_path,
                archive_path,
            } => {
                append_tar_entry(
                    &mut parts,
                    source_path,
                    &normalize_archive_path(archive_path)?,
                )?;
            }
            ArchiveEntry::Content {
                archive_path,
                content,
                mode,
                mtime_ms,
            } => {
                append_tar_file(
                    &mut parts,
                    &normalize_archive_path(archive_path)?,
                    content,
                    mode.unwrap_or(0o644),
                    mtime_ms.unwrap_or_else(current_mtime_ms),
                );
            }
        }
    }
    let metadata_content = format!(
        "{}\n",
        serde_json::to_string_pretty(&metadata).map_err(|error| error.to_string())?
    );
    append_tar_file(
        &mut parts,
        PLUGIN_SYNC_METADATA_ARCHIVE_PATH,
        metadata_content.as_bytes(),
        0o644,
        current_mtime_ms(),
    );
    parts.extend(std::iter::repeat_n(0u8, TAR_END_BLOCK_BYTES));
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(&parts)
        .map_err(|error| error.to_string())?;
    encoder.finish().map_err(|error| error.to_string())
}

pub fn extract_archive(
    archive: &[u8],
    target_dir: &Path,
    max_extracted_bytes: Option<u64>,
) -> Result<(), String> {
    std::fs::create_dir_all(target_dir).map_err(|error| error.to_string())?;
    let mut decoder = GzDecoder::new(archive);
    let mut data = Vec::new();
    if let Some(limit) = max_extracted_bytes {
        let cap = limit.saturating_add(TAR_END_BLOCK_BYTES as u64 + TAR_BLOCK_SIZE as u64 * 512);
        decoder
            .take(cap + 1)
            .read_to_end(&mut data)
            .map_err(|error| format!("plugin sync archive exceeds limit: {error}"))?;
        if data.len() as u64 > cap {
            return Err(format!(
                "plugin sync archive exceeds limit: {}/{}",
                data.len(),
                cap
            ));
        }
    } else {
        decoder
            .read_to_end(&mut data)
            .map_err(|error| error.to_string())?;
    }

    let mut offset = 0usize;
    let mut extracted_bytes: u64 = 0;
    while offset + TAR_BLOCK_SIZE <= data.len() {
        let header = &data[offset..offset + TAR_BLOCK_SIZE];
        offset += TAR_BLOCK_SIZE;
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        let size = read_tar_octal(header, 124, 12);
        let data_start = offset;
        let data_end = data_start + size;
        if data_end > data.len() {
            return Err("truncated plugin sync archive entry".to_string());
        }
        let payload = &data[data_start..data_end];
        offset = data_start + size.div_ceil(TAR_BLOCK_SIZE) * TAR_BLOCK_SIZE;

        let type_flag = read_tar_string(header, 156, 1).unwrap_or_else(|| "0".to_string());
        let archive_path = normalize_archive_path(&read_tar_entry_path(header))?;
        let target_path = resolve_within(target_dir, &archive_path)?;
        match type_flag.as_str() {
            "5" => {
                std::fs::create_dir_all(&target_path).map_err(|error| error.to_string())?;
            }
            "0" | "\0" => {
                extracted_bytes += size as u64;
                if let Some(limit) = max_extracted_bytes {
                    if extracted_bytes > limit {
                        return Err(format!(
                            "plugin sync archive exceeds limit: {extracted_bytes}/{limit}"
                        ));
                    }
                }
                if let Some(parent) = target_path.parent() {
                    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                std::fs::write(&target_path, payload).map_err(|error| error.to_string())?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = read_tar_mode(header);
                    let _ = std::fs::set_permissions(
                        &target_path,
                        std::fs::Permissions::from_mode(mode),
                    );
                }
            }
            other => {
                return Err(format!("unsupported plugin archive entry type: {other}"));
            }
        }
    }
    Ok(())
}

fn append_tar_entry(
    parts: &mut Vec<u8>,
    source_path: &Path,
    archive_path: &str,
) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(source_path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_dir() {
        let directory_path = ensure_trailing_slash(archive_path);
        parts.extend(create_tar_header(
            &directory_path,
            file_mode(&metadata, true),
            mtime_seconds(&metadata),
            0,
            '5',
        ));
        let mut children: Vec<PathBuf> = std::fs::read_dir(source_path)
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect();
        children.sort_by(|left, right| {
            plugin_discovery::js::locale_compare(
                &left.file_name().unwrap_or_default().to_string_lossy(),
                &right.file_name().unwrap_or_default().to_string_lossy(),
            )
        });
        for child in children {
            let name = child
                .file_name()
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_default();
            append_tar_entry(parts, &child, &format!("{archive_path}/{name}"))?;
        }
        return Ok(());
    }
    if !metadata.file_type().is_file() {
        return Err(format!(
            "unsupported plugin archive source: {}",
            source_path.display()
        ));
    }
    let content = std::fs::read(source_path).map_err(|error| error.to_string())?;
    append_tar_file(
        parts,
        archive_path,
        &content,
        file_mode(&metadata, false),
        mtime_seconds(&metadata) * 1000,
    );
    Ok(())
}

fn append_tar_file(
    parts: &mut Vec<u8>,
    archive_path: &str,
    content: &[u8],
    mode: u32,
    mtime_ms: u64,
) {
    let archive_path = archive_path.trim_end_matches('/').to_string();
    parts.extend(create_tar_header(
        &archive_path,
        mode,
        mtime_ms / 1000,
        content.len(),
        '0',
    ));
    parts.extend_from_slice(content);
    let padding = content.len().div_ceil(TAR_BLOCK_SIZE) * TAR_BLOCK_SIZE - content.len();
    parts.extend(std::iter::repeat_n(0u8, padding));
}

fn create_tar_header(
    entry_path: &str,
    mode: u32,
    mtime_seconds: u64,
    size: usize,
    type_flag: char,
) -> [u8; TAR_BLOCK_SIZE] {
    let mut header = [0u8; TAR_BLOCK_SIZE];
    let (name, prefix) = split_tar_entry_path(entry_path);
    write_tar_string(&mut header, &name, 0, 100);
    write_tar_octal(&mut header, (mode & 0o777) as u64, 100, 8);
    write_tar_octal(&mut header, 0, 108, 8);
    write_tar_octal(&mut header, 0, 116, 8);
    write_tar_octal(&mut header, size as u64, 124, 12);
    write_tar_octal(&mut header, mtime_seconds, 136, 12);
    header[148..156].fill(0x20);
    write_tar_string(&mut header, &type_flag.to_string(), 156, 1);
    write_tar_string(&mut header, "ustar", 257, 6);
    write_tar_string(&mut header, "00", 263, 2);
    write_tar_string(&mut header, "mode", 265, 32);
    write_tar_string(&mut header, "mode", 297, 32);
    write_tar_string(&mut header, &prefix, 345, 155);
    let checksum: u64 = header.iter().map(|byte| *byte as u64).sum();
    write_tar_checksum(&mut header, checksum);
    header
}

fn split_tar_entry_path(entry_path: &str) -> (String, String) {
    if entry_path.len() <= 100 {
        return (entry_path.to_string(), String::new());
    }
    let segments: Vec<&str> = entry_path.split('/').collect();
    for index in (1..segments.len()).rev() {
        let prefix = segments[..index].join("/");
        let name = segments[index..].join("/");
        if prefix.len() <= 155 && name.len() <= 100 {
            return (name, prefix);
        }
    }
    (entry_path.to_string(), String::new())
}

fn read_tar_entry_path(header: &[u8]) -> String {
    let name = read_tar_string(header, 0, 100).unwrap_or_default();
    let prefix = read_tar_string(header, 345, 155).unwrap_or_default();
    if prefix.is_empty() {
        name
    } else {
        format!("{prefix}/{name}")
    }
}

fn read_tar_string(buffer: &[u8], offset: usize, length: usize) -> Option<String> {
    let slice = buffer.get(offset..offset + length)?;
    let end = slice
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(slice.len());
    Some(String::from_utf8_lossy(&slice[..end]).into_owned())
}

fn read_tar_octal(buffer: &[u8], offset: usize, length: usize) -> usize {
    let raw = read_tar_string(buffer, offset, length).unwrap_or_default();
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        0
    } else {
        usize::from_str_radix(trimmed, 8).unwrap_or(0)
    }
}

#[cfg(unix)]
fn read_tar_mode(header: &[u8]) -> u32 {
    let mode = read_tar_octal(header, 100, 8) as u32;
    if mode == 0 {
        0o644
    } else {
        mode & 0o777
    }
}

fn write_tar_string(buffer: &mut [u8], value: &str, offset: usize, length: usize) {
    let source = value.as_bytes();
    let copied = source.len().min(length);
    buffer[offset..offset + copied].copy_from_slice(&source[..copied]);
}

fn write_tar_octal(buffer: &mut [u8], value: u64, offset: usize, length: usize) {
    let encoded = format!("{:0width$o}", value, width = length - 1);
    write_tar_string(buffer, &encoded, offset, length - 1);
    buffer[offset + length - 1] = 0;
}

fn write_tar_checksum(buffer: &mut [u8], checksum: u64) {
    let encoded = format!("{checksum:06o}");
    write_tar_string(buffer, &encoded, 148, 6);
    buffer[154] = 0;
    buffer[155] = 0x20;
}

fn normalize_archive_path(path: &str) -> Result<String, String> {
    normalize_relative_path(path).map_err(|_| format!("unsafe plugin archive path: {path}"))
}

fn ensure_trailing_slash(path: &str) -> String {
    if path.ends_with('/') {
        path.to_string()
    } else {
        format!("{path}/")
    }
}

fn current_mtime_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn mtime_seconds(metadata: &std::fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or_else(|| current_mtime_ms() / 1000)
}

fn file_mode(metadata: &std::fs::Metadata, is_dir: bool) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o777
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        if is_dir {
            0o777
        } else {
            0o666
        }
    }
}
