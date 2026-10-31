//! ustar 归档写读 + gzip（`skillSyncArchive.ts` 的逐字节对齐实现）。
//!
//! 只支持普通文件与目录（typeflag `0` / `5`），与 TS 一致；其它类型导入报错，
//! 导出遇到软链等非普通文件也报错。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;

use crate::paths::{normalize_skill_sync_relative_path, resolve_skill_sync_path_within};

const TAR_BLOCK_SIZE: usize = 512;
const TAR_END_BLOCK_BYTES: usize = TAR_BLOCK_SIZE * 2;

pub struct ArchiveEntry {
    pub source_path: PathBuf,
    pub archive_path: String,
}

pub fn create_archive(entries: &[ArchiveEntry]) -> Result<Vec<u8>, String> {
    let mut parts: Vec<u8> = Vec::new();
    for entry in entries {
        append_tar_entry(
            &mut parts,
            &entry.source_path,
            &normalize_archive_path(&entry.archive_path)?,
        )?;
    }
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
    decoder
        .read_to_end(&mut data)
        .map_err(|error| format!("invalid skill sync archive: {error}"))?;

    let mut offset = 0usize;
    let mut extracted_bytes: u64 = 0;
    while offset + TAR_BLOCK_SIZE <= data.len() {
        let header = &data[offset..offset + TAR_BLOCK_SIZE];
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        let size = read_tar_octal(header, 124, 12);
        if size < 0 {
            return Err("invalid skill sync archive entry size".to_string());
        }
        let size = size as u64;
        let type_flag = {
            let raw = read_tar_string(header, 156, 1);
            if raw.is_empty() {
                "0".to_string()
            } else {
                raw
            }
        };
        let archive_path = normalize_archive_path(&read_tar_entry_path(header))?;
        let target_path =
            resolve_skill_sync_path_within(target_dir, &archive_path, "unsafe skill archive path")?;
        offset += TAR_BLOCK_SIZE;

        if type_flag == "5" {
            offset += padded_bytes(size) as usize;
            std::fs::create_dir_all(&target_path).map_err(|error| error.to_string())?;
            continue;
        }
        if type_flag != "0" && type_flag != "\0" {
            return Err(format!("unsupported skill archive entry type: {type_flag}"));
        }
        let next_extracted = extracted_bytes + size;
        if max_extracted_bytes
            .map(|limit| next_extracted > limit)
            .unwrap_or(false)
        {
            return Err(size_limit_error(
                next_extracted,
                max_extracted_bytes.unwrap_or(0),
                "extracted-content",
            ));
        }
        if offset + size as usize > data.len() {
            return Err("truncated skill sync archive entry".to_string());
        }
        let content = &data[offset..offset + size as usize];
        offset += size as usize;
        offset += (padded_bytes(size) - size) as usize;
        if let Some(parent) = target_path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::write(&target_path, content).map_err(|error| error.to_string())?;
        extracted_bytes = next_extracted;
    }
    Ok(())
}

fn size_limit_error(actual: u64, max: u64, phase: &str) -> String {
    format!("skill sync size limit exceeded: {actual}/{max} ({phase})")
}

fn padded_bytes(size: u64) -> u64 {
    size.div_ceil(TAR_BLOCK_SIZE as u64) * TAR_BLOCK_SIZE as u64
}

fn append_tar_entry(
    parts: &mut Vec<u8>,
    source_path: &Path,
    archive_path: &str,
) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(source_path).map_err(|error| error.to_string())?;
    if metadata.is_dir() {
        let directory_entry = if archive_path.ends_with('/') {
            archive_path.to_string()
        } else {
            format!("{archive_path}/")
        };
        parts.extend(create_tar_header(
            &directory_entry,
            metadata_mode(&metadata),
            metadata_mtime_seconds(&metadata),
            0,
            "5",
        )?);
        let mut children: Vec<std::fs::DirEntry> = std::fs::read_dir(source_path)
            .map_err(|error| error.to_string())?
            .flatten()
            .collect();
        children.sort_by(|left, right| {
            plugin_discovery::js::locale_compare(
                &left.file_name().to_string_lossy(),
                &right.file_name().to_string_lossy(),
            )
        });
        for child in children {
            let child_archive = normalize_archive_path(&format!(
                "{}/{}",
                archive_path.trim_end_matches('/'),
                child.file_name().to_string_lossy()
            ))?;
            append_tar_entry(parts, &child.path(), &child_archive)?;
        }
        return Ok(());
    }
    if !metadata.is_file() {
        return Err(format!(
            "unsupported skill archive source: {}",
            source_path.to_string_lossy()
        ));
    }
    let content = std::fs::read(source_path).map_err(|error| error.to_string())?;
    let size = content.len() as u64;
    parts.extend(create_tar_header(
        archive_path,
        metadata_mode(&metadata),
        metadata_mtime_seconds(&metadata),
        size,
        "0",
    )?);
    parts.extend_from_slice(&content);
    parts.extend(std::iter::repeat_n(
        0u8,
        (padded_bytes(size) - size) as usize,
    ));
    Ok(())
}

#[cfg(unix)]
fn metadata_mode(metadata: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode()
}

#[cfg(not(unix))]
fn metadata_mode(metadata: &std::fs::Metadata) -> u32 {
    if metadata.is_dir() {
        0o777
    } else {
        0o666
    }
}

fn metadata_mtime_seconds(metadata: &std::fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn create_tar_header(
    entry_path: &str,
    mode: u32,
    mtime_seconds: u64,
    size: u64,
    type_flag: &str,
) -> Result<Vec<u8>, String> {
    let mut header = vec![0u8; TAR_BLOCK_SIZE];
    let (name, prefix) = split_tar_entry_path(entry_path)?;
    write_tar_string(&mut header, &name, 0, 100);
    write_tar_octal(&mut header, (mode & 0o777) as u64, 100, 8);
    write_tar_octal(&mut header, 0, 108, 8);
    write_tar_octal(&mut header, 0, 116, 8);
    write_tar_octal(&mut header, size, 124, 12);
    write_tar_octal(&mut header, mtime_seconds, 136, 12);
    for byte in &mut header[148..156] {
        *byte = 0x20;
    }
    write_tar_string(&mut header, type_flag, 156, 1);
    write_tar_string(&mut header, "ustar", 257, 6);
    write_tar_string(&mut header, "00", 263, 2);
    write_tar_string(&mut header, "mode", 265, 32);
    write_tar_string(&mut header, "mode", 297, 32);
    write_tar_string(&mut header, &prefix, 345, 155);
    let checksum: u64 = header.iter().map(|byte| *byte as u64).sum();
    let encoded = format!("{checksum:06o}");
    let encoded = encoded
        .chars()
        .rev()
        .take(6)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    write_tar_string(&mut header, &encoded, 148, 6);
    header[154] = 0;
    header[155] = 0x20;
    Ok(header)
}

fn split_tar_entry_path(entry_path: &str) -> Result<(String, String), String> {
    if entry_path.len() <= 100 {
        return Ok((entry_path.to_string(), String::new()));
    }
    let segments: Vec<&str> = entry_path.split('/').collect();
    for index in (1..segments.len()).rev() {
        let prefix = segments[..index].join("/");
        let name = segments[index..].join("/");
        if prefix.len() <= 155 && name.len() <= 100 {
            return Ok((name, prefix));
        }
    }
    Err(format!("skill archive path is too long: {entry_path}"))
}

fn read_tar_entry_path(header: &[u8]) -> String {
    let name = read_tar_string(header, 0, 100);
    let prefix = read_tar_string(header, 345, 155);
    if prefix.is_empty() {
        name
    } else {
        format!("{prefix}/{name}")
    }
}

fn read_tar_string(buffer: &[u8], offset: usize, length: usize) -> String {
    let slice = &buffer[offset..(offset + length).min(buffer.len())];
    let end = slice
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(slice.len());
    String::from_utf8_lossy(&slice[..end]).into_owned()
}

fn read_tar_octal(buffer: &[u8], offset: usize, length: usize) -> i64 {
    let raw = read_tar_string(buffer, offset, length);
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return 0;
    }
    i64::from_str_radix(trimmed, 8).unwrap_or(-1)
}

fn write_tar_string(buffer: &mut [u8], value: &str, offset: usize, length: usize) {
    let bytes = value.as_bytes();
    for (index, byte) in bytes.iter().take(length).enumerate() {
        if offset + index < buffer.len() {
            buffer[offset + index] = *byte;
        }
    }
}

fn write_tar_octal(buffer: &mut [u8], value: u64, offset: usize, length: usize) {
    let encoded = format!("{value:o}");
    let padded = format!("{:0>width$}", encoded, width = length - 1);
    let trimmed = if padded.len() > length - 1 {
        &padded[padded.len() - (length - 1)..]
    } else {
        &padded
    };
    write_tar_string(buffer, trimmed, offset, length - 1);
}

fn normalize_archive_path(path: &str) -> Result<String, String> {
    normalize_skill_sync_relative_path(path, "unsafe skill archive path")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tar_round_trip_and_gzip() {
        let root = std::env::temp_dir().join(format!(
            "zc-skill-sync-archive-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        let skill_dir = root.join("home/.mode/skills/alpha");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "---\nname: alpha\n---\nbody\n").unwrap();
        std::fs::write(skill_dir.join("note.txt"), "note\n").unwrap();

        let archive = create_archive(&[ArchiveEntry {
            source_path: skill_dir.clone(),
            archive_path: "alpha".to_string(),
        }])
        .unwrap();
        let target = root.join("extract");
        extract_archive(&archive, &target, None).unwrap();
        assert_eq!(
            std::fs::read_to_string(target.join("alpha/SKILL.md")).unwrap(),
            "---\nname: alpha\n---\nbody\n"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("alpha/note.txt")).unwrap(),
            "note\n"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn oversized_archive_is_rejected() {
        let root = std::env::temp_dir().join(format!(
            "zc-skill-sync-limit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        let skill_dir = root.join("alpha");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "x".repeat(1024)).unwrap();
        let archive = create_archive(&[ArchiveEntry {
            source_path: skill_dir,
            archive_path: "alpha".to_string(),
        }])
        .unwrap();
        let error = extract_archive(&archive, &root.join("out"), Some(10)).unwrap_err();
        assert!(error.starts_with("skill sync size limit exceeded"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
