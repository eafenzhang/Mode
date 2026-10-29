//! 线格式类型：与 `packages/shared/src/protocol.ts`、`file.ts` 的参数形状对齐。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryType {
    File,
    Directory,
}

impl EntryType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "directory",
        }
    }

    pub fn is_directory(self) -> bool {
        matches!(self, Self::Directory)
    }
}

impl Serialize for EntryType {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceFileEntry {
    pub name: String,
    pub path: String,
    pub relative_path: String,
    pub r#type: EntryType,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub r#type: EntryType,
    pub is_symbolic_link: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTextSlice {
    pub path: String,
    pub content: String,
    pub offset: u64,
    pub bytes_read: u64,
    pub total_bytes: u64,
    pub truncated: bool,
    pub is_binary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMediaPreview {
    pub path: String,
    pub media_type: String,
    pub data_base64: String,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileBinaryPreview {
    pub path: String,
    pub data_base64: String,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceFileSearchIgnoreContent {
    pub content: String,
    /// `file` | `template`
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceFileSearchIgnoreTransformResult {
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultWorkspace {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationWorkspace {
    pub path: String,
    pub created: bool,
    pub workspace_purpose: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileExistenceResult {
    pub path: String,
    pub exists: bool,
}

// ── 请求参数（ProxyChannel 的位置参数 0 是对象） ──

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathParams {
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReaddirParams {
    pub path: String,
    pub include_hidden: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckFilesExistParams {
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadTextFileParams {
    pub path: String,
    pub offset: Option<f64>,
    pub length: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRangeParams {
    pub path: String,
    pub offset: f64,
    pub length: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewParams {
    pub path: String,
    pub max_bytes: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceFileSearchParams {
    pub root_path: String,
    pub workspace_identity: Option<String>,
    pub query: String,
    pub limit: Option<f64>,
    pub refresh: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootParams {
    pub root_path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootRangeParams {
    pub root_path: String,
    pub offset: f64,
    pub length: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IgnoreTransformParams {
    pub root_path: String,
    pub transform: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteIgnoreParams {
    pub root_path: String,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScratchWorkspaceParams {
    pub name: String,
}

pub const TRANSFORM_SYNC_GITIGNORE: &str = "sync-gitignore";
pub const TRANSFORM_RESET_DEFAULTS: &str = "reset-defaults";
