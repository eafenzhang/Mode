//! `file` 频道的服务实现（W3-2 文件域）。
//!
//! 行为真源：
//! - `packages/services/src/file/fileService.ts`
//! - `packages/services/src/file/workspaceFileIgnore.ts`（`.modeignore`）
//! - `packages/services/src/file/workspaceFileMentionFilter.ts`（默认过滤）
//! - `packages/shared/src/workspaceFileEntriesCodec.ts`（列式打包）
//! - `packages/shared/src/workspaceFileSearch.ts`（候选映射与打分）
//!
//! 规范见 `docs/specs/file-service.md`。

mod base64;
mod codec;
mod error;
mod filter;
mod ignore;
mod index;
mod search;
mod service;
mod types;

pub use codec::{pack_entries, unpack_entries, WORKSPACE_FILE_ENTRIES_CHUNK_SIZE};
pub use error::{file_result, FileResult, FileServiceError};
pub use filter::{
    default_workspace_file_search_filter, node_extname, workspace_file_search_decision,
    WorkspaceFileSearchDecision,
};
pub use ignore::{
    is_workspace_file_search_path_ignored, load_workspace_file_search_ignore_rules,
    read_workspace_file_search_ignore, reset_workspace_file_search_ignore_defaults,
    resolve_workspace_ignore_path, sync_workspace_file_search_ignore_from_gitignore,
    transform_workspace_file_search_ignore, write_workspace_file_search_ignore, IgnoreRules,
    IgnoreSource, LEGACY_WORKSPACE_FILE_SEARCH_IGNORE_FILE_NAME,
    WORKSPACE_FILE_SEARCH_IGNORE_FILE_NAME,
};
pub use index::{utf16_len, WorkspaceFileIndex};
pub use search::{
    filter_workspace_file_search_candidates, map_entries_to_candidates,
    score_workspace_file_fuzzy_match, WorkspaceFileSearchCandidate,
    WORKSPACE_FILE_SEARCH_DISPLAY_CAP,
};
pub use service::{FileService, FileServiceOptions, WarnSink};
pub use types::*;
