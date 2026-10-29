//! Project Memory 只读服务（`memory` 频道的完整实现）。
//!
//! 行为真源：`packages/services/src/memory/memoryService.ts` 与
//! `projectMemoryStableRead.ts`。只有两个方法（`listProjectMemories` /
//! `readProjectMemoryFile`），无写入、无跨进程状态，因此作为 M4b 的第一个本地频道。
//!
//! 安全约束与 TS 一致：只读 `{dataRoot}/cli/memories/projects` 下、非符号链接的普通目录与
//! 普通文件；文件名必须**大小写精确**命中目录项；读取后校验文件未被替换或改写。
//!
//! ## 与 TS 的差异（Windows 上）
//!
//! TS 用 dev/ino/mtimeNs/ctimeNs 判定「同一文件快照」。Rust std 在 Windows 上拿不到稳定的
//! 文件索引，这里退化为 `size + mtime` 对比：仍能发现 rename 替换与原地覆盖（两者都会改
//! mtime），但同一时间戳精度内的同尺寸改写理论上可能漏判。Unix 上额外比较 dev/ino。

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Serialize;

pub const PROJECT_MEMORY_DIRECTORY_NAME: &str = "memory";
pub const PROJECT_MEMORY_INDEX_FILE_NAME: &str = "MEMORY.md";
pub const PROJECT_MEMORY_PREVIEW_LIMIT_EXCEEDED_ERROR_CODE: &str =
    "PROJECT_MEMORY_PREVIEW_LIMIT_EXCEEDED";
pub const PROJECT_MEMORY_FILE_CHANGED_ERROR_CODE: &str = "PROJECT_MEMORY_FILE_CHANGED";
const PROJECT_MEMORY_PREVIEW_MAX_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMemoryFileSummary {
    pub name: String,
    pub path: String,
    /// `"index"` | `"item"`
    pub kind: &'static str,
    pub size: u64,
    pub updated_at: u64,
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMemoryWorkspaceSummary {
    pub id: String,
    pub label: String,
    pub updated_at: u64,
    pub files: Vec<ProjectMemoryFileSummary>,
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMemoryFileContent {
    pub content: String,
    pub updated_at: u64,
}

#[derive(Debug)]
pub enum MemoryError {
    InvalidPath,
    NotRegularDirectory(PathBuf),
    NotRegularFile(PathBuf),
    FileNameMismatch(String),
    OutsideRoot(PathBuf),
    PreviewLimitExceeded(String),
    FileChanged(String),
    Io(std::io::Error),
}

impl MemoryError {
    /// 与 TS 的 `error.code` 对齐；`None` 表示普通错误。
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::PreviewLimitExceeded(_) => Some(PROJECT_MEMORY_PREVIEW_LIMIT_EXCEEDED_ERROR_CODE),
            Self::FileChanged(_) => Some(PROJECT_MEMORY_FILE_CHANGED_ERROR_CODE),
            _ => None,
        }
    }
}

impl std::fmt::Display for MemoryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPath => write!(formatter, "Invalid Project Memory path"),
            Self::NotRegularDirectory(path) => write!(
                formatter,
                "Project Memory directory is not a regular directory: {}",
                path.display()
            ),
            Self::NotRegularFile(path) => write!(
                formatter,
                "Project Memory file is not a regular file: {}",
                path.display()
            ),
            Self::FileNameMismatch(name) => write!(
                formatter,
                "Project Memory file name does not match exactly: {name}"
            ),
            Self::OutsideRoot(path) => write!(
                formatter,
                "Project Memory path is outside the local profile: {}",
                path.display()
            ),
            Self::PreviewLimitExceeded(name) => write!(
                formatter,
                "Project Memory file exceeds the 5 MiB preview limit: {name}"
            ),
            Self::FileChanged(name) => write!(
                formatter,
                "Project Memory file changed during preview: {name}"
            ),
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for MemoryError {}

impl From<std::io::Error> for MemoryError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

pub struct ProjectMemoryService {
    projects_root: PathBuf,
}

impl ProjectMemoryService {
    /// `data_root` 是 `{dataBaseDir}/.mode`（Host 的 `resolveDataRoot` 结果）。
    pub fn new(data_root: impl Into<PathBuf>) -> Self {
        Self {
            projects_root: Path::new(&data_root.into())
                .join("cli")
                .join("memories")
                .join("projects"),
        }
    }

    pub fn list_project_memories(&self) -> Result<Vec<ProjectMemoryWorkspaceSummary>, MemoryError> {
        let projects_root = match require_plain_directory(&self.projects_root) {
            Ok(path) => path,
            Err(MemoryError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Vec::new())
            }
            Err(error) => return Err(error),
        };

        let entries = match fs::read_dir(&projects_root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(MemoryError::Io(error)),
        };

        let mut workspaces = Vec::new();
        for entry in entries.flatten() {
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            if !file_type.is_dir() || file_type.is_symlink() {
                continue;
            }
            let workspace_id = entry.file_name().to_string_lossy().into_owned();
            let workspace_root = projects_root.join(&workspace_id);
            let memory_root = workspace_root.join(PROJECT_MEMORY_DIRECTORY_NAME);
            if !is_plain_directory(&workspace_root) || !is_plain_directory(&memory_root) {
                continue;
            }
            let memory_entries = match fs::read_dir(&memory_root) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(MemoryError::Io(error)),
            };

            let mut files = Vec::new();
            for memory_entry in memory_entries.flatten() {
                let name = memory_entry.file_name().to_string_lossy().into_owned();
                if !is_project_memory_file_name(&name) {
                    continue;
                }
                let file_type = match memory_entry.file_type() {
                    Ok(file_type) => file_type,
                    Err(_) => continue,
                };
                if !file_type.is_file() || file_type.is_symlink() {
                    continue;
                }
                let path = memory_root.join(&name);
                let metadata = match fs::symlink_metadata(&path) {
                    Ok(metadata) => metadata,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(MemoryError::Io(error)),
                };
                if !metadata.is_file() || metadata.file_type().is_symlink() {
                    continue;
                }
                files.push(ProjectMemoryFileSummary {
                    name: name.clone(),
                    path: path.to_string_lossy().into_owned(),
                    kind: if name == PROJECT_MEMORY_INDEX_FILE_NAME {
                        "index"
                    } else {
                        "item"
                    },
                    size: metadata.len(),
                    updated_at: modified_ms(&metadata),
                });
            }

            if files.is_empty() {
                continue;
            }
            files.sort_by(|left, right| {
                if left.kind != right.kind {
                    return if left.kind == "index" {
                        std::cmp::Ordering::Less
                    } else {
                        std::cmp::Ordering::Greater
                    };
                }
                left.name.cmp(&right.name)
            });
            let updated_at = files.iter().map(|file| file.updated_at).max().unwrap_or(0);
            workspaces.push(ProjectMemoryWorkspaceSummary {
                label: resolve_workspace_label(&workspace_id),
                id: workspace_id,
                updated_at,
                files,
            });
        }

        workspaces.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(workspaces)
    }

    pub fn read_project_memory_file(
        &self,
        workspace_id: &str,
        file_name: &str,
    ) -> Result<ProjectMemoryFileContent, MemoryError> {
        if !is_valid_path_segment(workspace_id)
            || !is_valid_path_segment(file_name)
            || !is_project_memory_file_name(file_name)
        {
            return Err(MemoryError::InvalidPath);
        }

        let projects_root = require_plain_directory(&self.projects_root)?;
        let workspace_root = projects_root.join(workspace_id);
        let memory_root = workspace_root.join(PROJECT_MEMORY_DIRECTORY_NAME);
        require_plain_directory(&workspace_root)?;
        require_plain_directory(&memory_root)?;

        let file_path = require_exact_file(&memory_root, file_name)?;
        ensure_contained(&projects_root, &file_path)?;

        let pre_open = fs::symlink_metadata(&file_path)?;
        if !pre_open.is_file() || pre_open.file_type().is_symlink() {
            return Err(MemoryError::NotRegularFile(file_path));
        }
        if pre_open.len() > PROJECT_MEMORY_PREVIEW_MAX_BYTES {
            return Err(MemoryError::PreviewLimitExceeded(file_name.to_string()));
        }

        let mut file = fs::File::open(&file_path)?;
        let opened = file.metadata()?;
        let post_open = fs::symlink_metadata(&file_path)?;
        if !opened.is_file()
            || !post_open.is_file()
            || post_open.file_type().is_symlink()
            || !same_snapshot(&opened, &pre_open)
            || !same_snapshot(&opened, &post_open)
        {
            return Err(MemoryError::FileChanged(file_name.to_string()));
        }

        let mut bytes = Vec::new();
        (&mut file)
            .take(PROJECT_MEMORY_PREVIEW_MAX_BYTES + 1)
            .read_to_end(&mut bytes)?;
        let final_metadata = file.metadata()?;
        if !same_snapshot(&opened, &final_metadata) {
            return Err(MemoryError::FileChanged(file_name.to_string()));
        }
        if bytes.len() as u64 > PROJECT_MEMORY_PREVIEW_MAX_BYTES
            || final_metadata.len() > PROJECT_MEMORY_PREVIEW_MAX_BYTES
        {
            return Err(MemoryError::PreviewLimitExceeded(file_name.to_string()));
        }

        let content = String::from_utf8_lossy(&bytes).into_owned();
        Ok(ProjectMemoryFileContent {
            content,
            updated_at: modified_ms(&final_metadata),
        })
    }
}

fn is_valid_path_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains('\\')
        && Path::new(value).file_name().map(|name| name == value).unwrap_or(false)
}

fn is_project_memory_file_name(name: &str) -> bool {
    name == PROJECT_MEMORY_INDEX_FILE_NAME || (name.ends_with(".md") && name != PROJECT_MEMORY_INDEX_FILE_NAME)
}

fn is_plain_directory(path: &Path) -> bool {
    match fs::symlink_metadata(path) {
        Ok(metadata) => metadata.is_dir() && !metadata.file_type().is_symlink(),
        Err(_) => false,
    }
}

fn require_plain_directory(path: &Path) -> Result<PathBuf, MemoryError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(MemoryError::NotRegularDirectory(path.to_path_buf()));
    }
    Ok(path.to_path_buf())
}

/// 目录项名与请求名必须**大小写精确**一致；大小写不敏感文件系统会让别名命中别的文件。
fn require_exact_file(memory_root: &Path, file_name: &str) -> Result<PathBuf, MemoryError> {
    for entry in fs::read_dir(memory_root)?.flatten() {
        if entry.file_name().to_string_lossy() == file_name {
            let file_type = entry.file_type()?;
            if !file_type.is_file() || file_type.is_symlink() {
                return Err(MemoryError::NotRegularFile(memory_root.join(file_name)));
            }
            return Ok(memory_root.join(file_name));
        }
    }
    Err(MemoryError::Io(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("Project Memory file not found: {file_name}"),
    )))
}

/// 真实路径必须落在 projects 根目录内（防 symlink 逃逸）。
fn ensure_contained(projects_root: &Path, target: &Path) -> Result<(), MemoryError> {
    let root_real = fs::canonicalize(projects_root)?;
    let target_real = fs::canonicalize(target)?;
    if !target_real.starts_with(&root_real) {
        return Err(MemoryError::OutsideRoot(target.to_path_buf()));
    }
    Ok(())
}

/// `^(.*)-[a-f0-9]{16}$`（大小写不敏感）：取最后 17 个字符判断。
fn resolve_workspace_label(workspace_id: &str) -> String {
    let bytes = workspace_id.as_bytes();
    if bytes.len() >= 17 {
        let split = bytes.len() - 17;
        if bytes[split] == b'-' && bytes[split + 1..].iter().all(u8::is_ascii_hexdigit) {
            let slug = workspace_id[..split].trim();
            if !slug.is_empty() {
                return slug.to_string();
            }
        }
    }
    workspace_id.to_string()
}

fn modified_ms(metadata: &fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// 同一文件快照。Unix 上额外比较 dev/ino；Windows 上只有 size + mtime 可用。
fn same_snapshot(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if left.dev() != right.dev()
            || left.ino() != right.ino()
            || left.size() != right.size()
            || left.mtime_nsec() != right.mtime_nsec()
            || left.ctime_nsec() != right.ctime_nsec()
        {
            return false;
        }
        true
    }
    #[cfg(not(unix))]
    {
        left.len() == right.len() && left.modified().ok() == right.modified().ok()
    }
}
