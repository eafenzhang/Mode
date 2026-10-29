//! `file` 服务的 17 个方法。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::base64::encode_base64;
use crate::codec::unpack_entries;
use crate::error::{file_result, FileResult};
use crate::filter::node_extname;
use crate::ignore;
use crate::index::{compare_names, modified_ms, utf16_len, WorkspaceFileIndex};
use crate::search::{
    filter_workspace_file_search_candidates, map_entries_to_candidates,
    WORKSPACE_FILE_SEARCH_DISPLAY_CAP,
};
use crate::types::*;

const DEFAULT_TEXT_READ_BYTES: u64 = 128 * 1024;
const MAX_TEXT_READ_BYTES: u64 = 256 * 1024;
const DEFAULT_MEDIA_PREVIEW_BYTES: u64 = 4 * 1024 * 1024;
const MAX_MEDIA_PREVIEW_BYTES: u64 = 8 * 1024 * 1024;
const DEFAULT_BINARY_READ_BYTES: u64 = 256 * 1024;
const MAX_BINARY_READ_BYTES: u64 = 1024 * 1024;
const DEFAULT_BINARY_PREVIEW_BYTES: u64 = 25 * 1024 * 1024;
const MAX_BINARY_PREVIEW_BYTES: u64 = 25 * 1024 * 1024;
const FILE_EXISTENCE_CACHE_TTL: Duration = Duration::from_secs(60);
const FILE_EXISTENCE_CACHE_MAX_ENTRIES: usize = 100;
const FILE_EXISTENCE_BATCH_LIMIT: usize = 15;

const IMAGE_EXTENSION_TO_MEDIA_TYPE: &[(&str, &str)] = &[
    (".apng", "image/apng"),
    (".avif", "image/avif"),
    (".bmp", "image/bmp"),
    (".gif", "image/gif"),
    (".ico", "image/x-icon"),
    (".jpeg", "image/jpeg"),
    (".jpg", "image/jpeg"),
    (".png", "image/png"),
    (".svg", "image/svg+xml"),
    (".webp", "image/webp"),
];

/// 与 `packages/shared/src/media-preview.ts` 同步（只用到 mediaType）。
const MEDIA_PREVIEW_FORMATS: &[(&str, &str)] = &[
    (".mp4", "video/mp4"),
    (".mov", "video/quicktime"),
    (".webm", "video/webm"),
    (".m4v", "video/x-m4v"),
    (".mp3", "audio/mpeg"),
    (".wav", "audio/wav"),
    (".m4a", "audio/mp4"),
    (".ogg", "audio/ogg"),
    (".opus", "audio/opus"),
    (".flac", "audio/flac"),
    (".weba", "audio/webm"),
];

pub type WarnSink = Arc<dyn Fn(&str) + Send + Sync>;

pub struct FileServiceOptions {
    /// `{dataBaseDir}/.mode`（壳侧 `resolve_data_root`）。
    pub data_root: PathBuf,
    /// `os.homedir()`；`ModeProject` 默认工作区建在这里。
    pub home: PathBuf,
    pub on_warn: Option<WarnSink>,
}

impl FileServiceOptions {
    pub fn from_process(data_root: impl Into<PathBuf>) -> Self {
        Self {
            data_root: data_root.into(),
            home: PathBuf::from(node_homedir()),
            on_warn: None,
        }
    }
}

struct ExistenceEntry {
    path: String,
    exists: bool,
    expires_at: Instant,
}

#[derive(Default)]
struct ExistenceCache {
    entries: VecDeque<ExistenceEntry>,
}

impl ExistenceCache {
    fn get(&mut self, path: &str) -> Option<bool> {
        let position = self.entries.iter().position(|entry| entry.path == path)?;
        let entry = self.entries.remove(position)?;
        if entry.expires_at <= Instant::now() {
            return None;
        }
        let exists = entry.exists;
        self.entries.push_back(entry);
        Some(exists)
    }

    fn set(&mut self, path: &str, exists: bool) {
        let now = Instant::now();
        self.entries.retain(|entry| entry.expires_at > now);
        if let Some(position) = self.entries.iter().position(|entry| entry.path == path) {
            self.entries.remove(position);
        }
        self.entries.push_back(ExistenceEntry {
            path: path.to_string(),
            exists,
            expires_at: now + FILE_EXISTENCE_CACHE_TTL,
        });
        while self.entries.len() > FILE_EXISTENCE_CACHE_MAX_ENTRIES {
            self.entries.pop_front();
        }
    }
}

pub struct FileService {
    data_root: PathBuf,
    home: PathBuf,
    index: WorkspaceFileIndex,
    on_warn: Option<WarnSink>,
    existence: Mutex<ExistenceCache>,
}

impl FileService {
    pub fn new(options: FileServiceOptions) -> Self {
        Self {
            data_root: options.data_root,
            home: options.home,
            index: WorkspaceFileIndex::default(),
            on_warn: options.on_warn,
            existence: Mutex::new(ExistenceCache::default()),
        }
    }

    pub async fn search_workspace_files(
        &self,
        params: &WorkspaceFileSearchParams,
    ) -> FileResult<Vec<WorkspaceFileEntry>> {
        let requested_limit = params
            .limit
            .unwrap_or(WORKSPACE_FILE_SEARCH_DISPLAY_CAP as f64);
        if !requested_limit.is_finite() {
            return Err(file_result("Invalid workspace file search query or limit"));
        }
        let limit = requested_limit
            .trunc()
            .clamp(0.0, WORKSPACE_FILE_SEARCH_DISPLAY_CAP as f64) as usize;
        if limit == 0 {
            return Ok(Vec::new());
        }

        let index = self
            .index
            .ensure(
                &params.root_path,
                params.workspace_identity.as_deref(),
                params.refresh.unwrap_or(false),
                self.on_warn.as_deref(),
            )
            .await?;
        let candidates = if let Some(candidates) = index.candidates.get() {
            candidates
        } else {
            let entries = unpack_entries(&index.packed, &params.root_path);
            let _ = index.candidates.set(map_entries_to_candidates(&entries));
            index
                .candidates
                .get()
                .expect("候选集刚写入，读取必然成功")
        };

        Ok(
            filter_workspace_file_search_candidates(candidates, &params.query, Some(limit), false)
                .into_iter()
                .map(|candidate| WorkspaceFileEntry {
                    name: candidate.name,
                    path: candidate.path,
                    relative_path: candidate.relative_path,
                    r#type: candidate.r#type,
                })
                .collect(),
        )
    }

    pub async fn readdir(&self, params: &ReaddirParams) -> FileResult<Vec<FileEntry>> {
        let include_hidden = params.include_hidden.unwrap_or(false);
        let mut reader = fs::read_dir(&params.path)
            .await
            .map_err(|error| file_result(error.to_string()))?;
        let mut entries = Vec::new();
        while let Some(entry) = reader
            .next_entry()
            .await
            .map_err(|error| file_result(error.to_string()))?
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !include_hidden && name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            let file_type = entry
                .file_type()
                .await
                .map_err(|error| file_result(error.to_string()))?;
            let is_symbolic_link = file_type.is_symlink();
            let entry_type = if file_type.is_dir() {
                EntryType::Directory
            } else if !is_symbolic_link {
                EntryType::File
            } else {
                match fs::metadata(&path).await {
                    Ok(metadata) if metadata.is_dir() => EntryType::Directory,
                    _ => EntryType::File,
                }
            };
            entries.push(FileEntry {
                name,
                path: path.to_string_lossy().into_owned(),
                r#type: entry_type,
                is_symbolic_link,
            });
        }
        entries.sort_by(|left, right| match (left.r#type, right.r#type) {
            (EntryType::Directory, EntryType::File) => std::cmp::Ordering::Less,
            (EntryType::File, EntryType::Directory) => std::cmp::Ordering::Greater,
            _ => compare_names(&left.name, &right.name),
        });
        Ok(entries)
    }

    pub async fn stat(&self, params: &PathParams) -> FileResult<serde_json::Value> {
        let metadata = fs::metadata(&params.path)
            .await
            .map_err(|error| file_result(error.to_string()))?;
        if metadata.is_dir() {
            return Ok(serde_json::json!({
                "path": params.path,
                "type": "directory",
            }));
        }
        Ok(serde_json::json!({
            "path": params.path,
            "type": "file",
            "size": metadata.len(),
            "mtimeMs": modified_ms(&metadata),
        }))
    }

    pub async fn check_files_exist(
        &self,
        params: &CheckFilesExistParams,
    ) -> FileResult<Vec<FileExistenceResult>> {
        if params.paths.len() > FILE_EXISTENCE_BATCH_LIMIT {
            return Err(file_result(format!(
                "File existence check supports at most {FILE_EXISTENCE_BATCH_LIMIT} paths."
            )));
        }
        let mut results = Vec::with_capacity(params.paths.len());
        for path in &params.paths {
            results.push(FileExistenceResult {
                path: path.clone(),
                exists: self.check_file_exists(path).await,
            });
        }
        Ok(results)
    }

    async fn check_file_exists(&self, path: &str) -> bool {
        if let Some(exists) = self.existence.lock().unwrap().get(path) {
            return exists;
        }
        let exists = fs::metadata(path)
            .await
            .map(|metadata| metadata.is_file())
            .unwrap_or(false);
        self.existence.lock().unwrap().set(path, exists);
        exists
    }

    pub async fn resolve_path(&self, params: &PathParams) -> FileResult<String> {
        let resolved = fs::canonicalize(&params.path)
            .await
            .map_err(|error| file_result(error.to_string()))?;
        Ok(strip_verbatim_prefix(&resolved.to_string_lossy()))
    }

    pub async fn ensure_conversation_workspace(&self) -> FileResult<ConversationWorkspace> {
        let path = self.data_root.join("workspace").join("default");
        let created = !path.exists();
        if let Err(error) = fs::create_dir_all(&path).await {
            match fs::metadata(&path).await {
                Ok(metadata) if metadata.is_dir() => {}
                _ => return Err(file_result(error.to_string())),
            }
        }
        let metadata = fs::metadata(&path)
            .await
            .map_err(|error| file_result(error.to_string()))?;
        if !metadata.is_dir() {
            return Err(file_result(format!(
                "Workspace path is not a directory: {}",
                path.display()
            )));
        }
        Ok(ConversationWorkspace {
            path: path.to_string_lossy().into_owned(),
            created,
            workspace_purpose: "conversation".into(),
        })
    }

    pub async fn create_default_workspace(&self) -> FileResult<DefaultWorkspace> {
        let path = self.home.join("ModeProject");
        create_workspace_directory(&path).await?;
        Ok(DefaultWorkspace {
            path: path.to_string_lossy().into_owned(),
        })
    }

    pub async fn create_scratch_workspace(
        &self,
        params: &ScratchWorkspaceParams,
    ) -> FileResult<DefaultWorkspace> {
        let name = params.name.trim();
        if name.is_empty() {
            return Err(file_result("Workspace name is required."));
        }
        if name.contains('/') || name.contains('\\') {
            return Err(file_result("Workspace name cannot contain path separators."));
        }
        let path = self.home.join("ModeProject").join(name);
        create_workspace_directory(&path).await?;
        Ok(DefaultWorkspace {
            path: path.to_string_lossy().into_owned(),
        })
    }

    pub async fn read_text_file(&self, params: &ReadTextFileParams) -> FileResult<FileTextSlice> {
        let metadata = fs::metadata(&params.path)
            .await
            .map_err(|error| file_result(error.to_string()))?;
        if !metadata.is_file() {
            return Err(file_result(format!("Path is not a file: {}", params.path)));
        }
        let total_bytes = metadata.len();
        let offset = trunc_non_negative(params.offset.unwrap_or(0.0));
        if offset >= total_bytes {
            return Ok(FileTextSlice {
                path: params.path.clone(),
                content: String::new(),
                offset,
                bytes_read: 0,
                total_bytes,
                truncated: false,
                is_binary: false,
            });
        }

        let target_length = clamp_read_length(
            params.length,
            DEFAULT_TEXT_READ_BYTES,
            MAX_TEXT_READ_BYTES,
        );
        let read_length = target_length.min(total_bytes - offset);
        let chunk = read_at(&params.path, offset, read_length).await?;
        let bytes_read = chunk.len() as u64;
        let is_binary = is_probably_binary(&chunk);
        Ok(FileTextSlice {
            path: params.path.clone(),
            content: if is_binary {
                String::new()
            } else {
                String::from_utf8_lossy(&chunk).into_owned()
            },
            offset,
            bytes_read,
            total_bytes,
            truncated: offset + bytes_read < total_bytes,
            is_binary,
        })
    }

    pub async fn read_file_range(&self, params: &FileRangeParams) -> FileResult<Vec<u8>> {
        let metadata = fs::metadata(&params.path)
            .await
            .map_err(|error| file_result(error.to_string()))?;
        if !metadata.is_file() {
            return Err(file_result(format!("Path is not a file: {}", params.path)));
        }
        let total_bytes = metadata.len();
        let offset = trunc_non_negative(params.offset);
        if offset >= total_bytes {
            return Ok(Vec::new());
        }
        let target_length = clamp_read_length(
            Some(params.length),
            DEFAULT_BINARY_READ_BYTES,
            MAX_BINARY_READ_BYTES,
        );
        let read_length = target_length.min(total_bytes - offset);
        read_at(&params.path, offset, read_length).await
    }

    pub async fn read_media_preview(
        &self,
        params: &PreviewParams,
    ) -> FileResult<FileMediaPreview> {
        let metadata = fs::metadata(&params.path)
            .await
            .map_err(|error| file_result(error.to_string()))?;
        if !metadata.is_file() {
            return Err(file_result(format!("Path is not a file: {}", params.path)));
        }
        let max_bytes = clamp_read_length(
            params.max_bytes,
            DEFAULT_MEDIA_PREVIEW_BYTES,
            MAX_MEDIA_PREVIEW_BYTES,
        );
        if metadata.len() > max_bytes {
            return Err(file_result(format!(
                "File is too large to preview: {}",
                params.path
            )));
        }
        let content = fs::read(&params.path)
            .await
            .map_err(|error| file_result(error.to_string()))?;
        Ok(FileMediaPreview {
            path: params.path.clone(),
            media_type: infer_media_type(&params.path),
            data_base64: encode_base64(&content),
            total_bytes: metadata.len(),
        })
    }

    pub async fn read_binary_preview(&self, params: &PreviewParams) -> FileResult<FileBinaryPreview> {
        let metadata = fs::metadata(&params.path)
            .await
            .map_err(|error| file_result(error.to_string()))?;
        if !metadata.is_file() {
            return Err(file_result(format!("Path is not a file: {}", params.path)));
        }
        let max_bytes = clamp_read_length(
            params.max_bytes,
            DEFAULT_BINARY_PREVIEW_BYTES,
            MAX_BINARY_PREVIEW_BYTES,
        );
        if metadata.len() > max_bytes {
            return Err(file_result(format!(
                "File is too large to preview: {}",
                params.path
            )));
        }
        let content = fs::read(&params.path)
            .await
            .map_err(|error| file_result(error.to_string()))?;
        Ok(FileBinaryPreview {
            path: params.path.clone(),
            data_base64: encode_base64(&content),
            total_bytes: metadata.len(),
        })
    }

    pub async fn list_workspace_files_length(&self, params: &RootParams) -> FileResult<usize> {
        let index = self
            .index
            .ensure(&params.root_path, None, false, self.on_warn.as_deref())
            .await?;
        // JS String.length 是 UTF-16 码元数，渲染层按同一口径切块。
        Ok(utf16_len(&index.packed))
    }

    pub async fn list_workspace_files_range(&self, params: &RootRangeParams) -> FileResult<String> {
        let index = self
            .index
            .ensure(&params.root_path, None, false, self.on_warn.as_deref())
            .await?;
        let total = utf16_len(&index.packed);
        let offset = trunc_non_negative(params.offset) as usize;
        if offset >= total {
            return Ok(String::new());
        }
        let length = trunc_non_negative(params.length) as usize;
        Ok(slice_utf16(&index.packed, offset, length))
    }

    pub async fn read_workspace_file_search_ignore(
        &self,
        params: &RootParams,
    ) -> FileResult<WorkspaceFileSearchIgnoreContent> {
        ignore::read_workspace_file_search_ignore(Path::new(&params.root_path)).await
    }

    pub async fn apply_workspace_file_search_ignore_transform(
        &self,
        params: &IgnoreTransformParams,
    ) -> FileResult<WorkspaceFileSearchIgnoreTransformResult> {
        let content =
            ignore::transform_workspace_file_search_ignore(Path::new(&params.root_path), &params.transform)
                .await?;
        Ok(WorkspaceFileSearchIgnoreTransformResult { content })
    }

    pub async fn write_workspace_file_search_ignore(
        &self,
        params: &WriteIgnoreParams,
    ) -> FileResult<()> {
        ignore::write_workspace_file_search_ignore(Path::new(&params.root_path), &params.content).await
    }
}

async fn create_workspace_directory(path: &Path) -> FileResult<()> {
    fs::create_dir_all(path)
        .await
        .map_err(|error| file_result(error.to_string()))?;
    let metadata = fs::metadata(path)
        .await
        .map_err(|error| file_result(error.to_string()))?;
    if !metadata.is_dir() {
        return Err(file_result(format!(
            "Workspace path is not a directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn trunc_non_negative(value: f64) -> u64 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    value.trunc().min(u64::MAX as f64) as u64
}

fn clamp_read_length(value: Option<f64>, default: u64, max: u64) -> u64 {
    match value.filter(|value| value.is_finite()) {
        Some(value) => (value.trunc().max(1.0) as u64).min(max),
        None => default,
    }
}

async fn read_at(path: &str, offset: u64, length: u64) -> FileResult<Vec<u8>> {
    let mut file = fs::File::open(path)
        .await
        .map_err(|error| file_result(error.to_string()))?;
    file.seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|error| file_result(error.to_string()))?;
    let mut buffer = vec![0u8; length as usize];
    let mut filled = 0usize;
    while filled < buffer.len() {
        let read = file
            .read(&mut buffer[filled..])
            .await
            .map_err(|error| file_result(error.to_string()))?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    buffer.truncate(filled);
    Ok(buffer)
}

/// `isProbablyBinary`：空文件按文本；NUL 直接判二进制；控制字符占比 > 30% 判二进制。
fn is_probably_binary(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let mut suspicious = 0usize;
    for &value in bytes {
        if value == 0 {
            return true;
        }
        let is_common_whitespace = matches!(value, 9 | 10 | 12 | 13);
        let is_control = (1..=8).contains(&value) || (14..=31).contains(&value) || value == 127;
        if !is_common_whitespace && is_control {
            suspicious += 1;
        }
    }
    suspicious as f64 / bytes.len() as f64 > 0.3
}

fn infer_media_type(path: &str) -> String {
    let extension = node_extname(path).to_lowercase();
    if let Some((_, media_type)) = IMAGE_EXTENSION_TO_MEDIA_TYPE
        .iter()
        .find(|(candidate, _)| *candidate == extension)
    {
        return (*media_type).to_string();
    }
    let normalized = path.replace('\\', "/").to_lowercase();
    if let Some((_, media_type)) = MEDIA_PREVIEW_FORMATS
        .iter()
        .find(|(extension, _)| normalized.ends_with(extension))
    {
        return (*media_type).to_string();
    }
    "application/octet-stream".to_string()
}

/// UTF-16 码元切片；字符跨边界时整字符丢弃（JS slice 可产生孤立代理项，Rust 字符串无法表示）。
fn slice_utf16(text: &str, start: usize, length: usize) -> String {
    let end = start.saturating_add(length);
    let mut out = String::new();
    let mut position = 0usize;
    for ch in text.chars() {
        let next = position + ch.len_utf16();
        if next <= start {
            position = next;
            continue;
        }
        if position >= end {
            break;
        }
        if position >= start && next <= end {
            out.push(ch);
        }
        position = next;
    }
    out
}

/// `fs::canonicalize` 在 Windows 上返回 `\\?\C:\...`；Node `realpath` 不带该前缀。
fn strip_verbatim_prefix(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    if let Some(rest) = path.strip_prefix(r"\\?\") {
        return rest.to_string();
    }
    path.to_string()
}

/// `os.homedir()`：Windows `USERPROFILE` → `HOMEDRIVE`+`HOMEPATH`；Unix `HOME`。
fn node_homedir() -> String {
    if let Some(profile) = std::env::var("USERPROFILE").ok().filter(|value| !value.is_empty()) {
        return profile;
    }
    if let (Ok(drive), Ok(home_path)) = (std::env::var("HOMEDRIVE"), std::env::var("HOMEPATH")) {
        if !drive.is_empty() && !home_path.is_empty() {
            return format!("{drive}{home_path}");
        }
    }
    std::env::var("HOME").unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_character_ratio_decides_binary() {
        assert!(!is_probably_binary(b""));
        assert!(!is_probably_binary("中文文本\n".as_bytes()));
        assert!(is_probably_binary(&[0, 1, 2]));
        assert!(is_probably_binary(&[1, 2, 3, 4, 65, 66]));
        assert!(!is_probably_binary(&[1, 65, 66, 67, 68, 69, 70, 71, 72, 73]));
    }

    #[test]
    fn media_type_prefers_image_table_then_media_table() {
        assert_eq!(infer_media_type("/tmp/a.PNG"), "image/png");
        assert_eq!(infer_media_type("C:\\x\\clip.MP4"), "video/mp4");
        assert_eq!(infer_media_type("/tmp/a.txt"), "application/octet-stream");
    }

    #[test]
    fn utf16_slicing_matches_js_boundaries() {
        let text = "ab中文c";
        assert_eq!(slice_utf16(text, 0, 2), "ab");
        assert_eq!(slice_utf16(text, 2, 2), "中文");
        assert_eq!(slice_utf16(text, 4, 10), "c");
        assert_eq!(slice_utf16(text, 5, 10), "");
    }

    #[test]
    fn verbatim_prefix_is_stripped() {
        assert_eq!(strip_verbatim_prefix(r"\\?\C:\ws\a"), r"C:\ws\a");
        assert_eq!(strip_verbatim_prefix(r"\\?\UNC\server\share\a"), r"\\server\share\a");
        assert_eq!(strip_verbatim_prefix("/ws/a"), "/ws/a");
    }
}
