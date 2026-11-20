//! v4 附件状态（C4 T4）：begin/chunk/commit/abort 上行事务与 ref 读授权。
//!
//! 观察基线：`docs/specs/agent-c4-contract.md` §5 与
//! `fixtures/c4/attachments-v4-frames.json`。授权按「正确行为」实现（C4-T4 裁决
//! 2026-11-20：不复刻 oracle 的 FK/previewRef 缺陷）：
//!
//! - `v4/attachment/*`：uploadId 必须存在且属于同一 session；
//! - `ref` 读面：ref 属于当前 session → 授权；跨 session → share 错误；未知 → preview 错误。
//!
//! 落盘：`<dataRoot>/cli/artifacts/<sessionId>/prompt-attachment-upload-<uploadId>-tool-result-<uuid><ext>`。

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Mutex;

use base64::Engine;
use sha2::{Digest, Sha256};

/// 已落盘的附件（commit 后按 ref 可读）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredAttachment {
    pub session_id: String,
    pub path: PathBuf,
    pub media_type: String,
    pub total_bytes: u64,
    pub file_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Upload {
    Staging {
        session_id: String,
        file_name: String,
        media_type: String,
        total_bytes: u64,
        total_chunks: u64,
        chunks: BTreeMap<u64, Vec<u8>>,
    },
    Committed {
        session_id: String,
        attachment_ref: String,
    },
}

/// 附件事务的错误分类（fault 字符串在 methods 层映射成 RPC 错误）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachmentError {
    /// commit/abort/chunk 的 uploadId 不存在（含 abort 后再 commit）。
    UploadNotFound,
    /// chunk/commit/abort 的 uploadId 属于别的 session。
    SessionMismatch,
    /// chunk_index 越界（>= totalChunks）或重复。
    ChunkIndexInvalid,
    /// commit 时块不齐。
    ChunksIncomplete { missing: Vec<u64> },
    /// 落盘失败。
    Io(String),
}

impl AttachmentError {
    pub fn fault(&self) -> &'static str {
        match self {
            Self::UploadNotFound => "fault.attachment.uploadNotFound",
            Self::SessionMismatch => "fault.attachment.sessionMismatch",
            Self::ChunkIndexInvalid => "fault.attachment.chunkIndexInvalid",
            Self::ChunksIncomplete { .. } => "fault.attachment.chunksIncomplete",
            Self::Io(_) => "fault.attachment.io",
        }
    }
}

/// ref 读面的授权结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefAuthorization {
    /// 同 session：授权。
    Authorized(StoredAttachment),
    /// ref 存在但属于别的 session（share 面）。
    Foreign(StoredAttachment),
    /// ref 未知（preview 面）。
    Unknown,
}

#[derive(Default)]
pub struct AttachmentStore {
    uploads: Mutex<HashMap<String, Upload>>,
    refs: Mutex<HashMap<String, StoredAttachment>>,
}

impl AttachmentStore {
    /// begin：新上传进 staging；已 commit 的同 id 幂等重现 committed 分支。
    pub fn begin(
        &self,
        upload_id: &str,
        session_id: &str,
        file_name: &str,
        media_type: &str,
        total_bytes: u64,
        total_chunks: u64,
    ) -> Result<BeginOutcome, AttachmentError> {
        let mut uploads = self
            .uploads
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match uploads.get(upload_id) {
            Some(Upload::Committed {
                session_id: owner,
                attachment_ref,
            }) => {
                if owner != session_id {
                    return Err(AttachmentError::SessionMismatch);
                }
                return Ok(BeginOutcome::Committed(attachment_ref.clone()));
            }
            Some(Upload::Staging { .. }) => {
                // 幂等重现 staging；不重置已收块。
                return Ok(BeginOutcome::Staging);
            }
            None => {}
        }
        uploads.insert(
            upload_id.to_string(),
            Upload::Staging {
                session_id: session_id.to_string(),
                file_name: file_name.to_string(),
                media_type: media_type.to_string(),
                total_bytes,
                total_chunks,
                chunks: BTreeMap::new(),
            },
        );
        Ok(BeginOutcome::Staging)
    }

    /// chunk：追加一块；返回 next_chunk_index。
    pub fn chunk(
        &self,
        upload_id: &str,
        session_id: &str,
        chunk_index: u64,
        data_base64: &str,
    ) -> Result<u64, AttachmentError> {
        use base64::engine::general_purpose::STANDARD as BASE64;
        let data = BASE64
            .decode(data_base64)
            .map_err(|error| AttachmentError::Io(error.to_string()))?;
        let mut uploads = self
            .uploads
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(Upload::Staging {
            session_id: owner,
            total_chunks,
            chunks,
            ..
        }) = uploads.get_mut(upload_id)
        else {
            return Err(AttachmentError::UploadNotFound);
        };
        if owner != session_id {
            return Err(AttachmentError::SessionMismatch);
        }
        if chunk_index >= *total_chunks || chunks.contains_key(&chunk_index) {
            return Err(AttachmentError::ChunkIndexInvalid);
        }
        chunks.insert(chunk_index, data);
        Ok(chunk_index + 1)
    }

    /// commit：块齐后落盘并登记 ref；返回 `(ref, 落盘事实)`。
    pub fn commit(
        &self,
        upload_id: &str,
        session_id: &str,
        data_root: &std::path::Path,
    ) -> Result<(String, StoredAttachment), AttachmentError> {
        let bytes = {
            let uploads = self
                .uploads
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(upload) = uploads.get(upload_id) else {
                return Err(AttachmentError::UploadNotFound);
            };
            let Upload::Staging {
                session_id: owner,
                file_name: _,
                media_type: _,
                total_chunks,
                chunks,
                ..
            } = upload
            else {
                // 幂等 commit：返回已登记的 ref 条目。
                let Upload::Committed {
                    session_id: owner,
                    attachment_ref,
                } = upload
                else {
                    unreachable!("upload 只有两态");
                };
                if owner != session_id {
                    return Err(AttachmentError::SessionMismatch);
                }
                let refs = self
                    .refs
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let stored = refs
                    .get(attachment_ref)
                    .cloned()
                    .ok_or(AttachmentError::UploadNotFound)?;
                return Ok((attachment_ref.clone(), stored));
            };
            if owner != session_id {
                return Err(AttachmentError::SessionMismatch);
            }
            let missing: Vec<u64> = (0..*total_chunks)
                .filter(|index| !chunks.contains_key(index))
                .collect();
            if !missing.is_empty() {
                return Err(AttachmentError::ChunksIncomplete { missing });
            }
            let bytes: Vec<u8> = chunks.values().flatten().cloned().collect();
            bytes
        };
        let attachment_ref = format!(
            "mode-artifact://{session_id}/tool-result-{}",
            uuid::Uuid::new_v4()
        );
        let file_name = {
            let uploads = self
                .uploads
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(Upload::Staging {
                file_name,
                media_type,
                ..
            }) = uploads.get(upload_id)
            else {
                return Err(AttachmentError::UploadNotFound);
            };
            (file_name.clone(), media_type.clone())
        };
        let target = artifact_path(data_root, session_id, upload_id, &file_name.0);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| AttachmentError::Io(error.to_string()))?;
        }
        std::fs::write(&target, &bytes).map_err(|error| AttachmentError::Io(error.to_string()))?;
        // PNG 另进 image-cache（观察布局；内容哈希命名）。
        if file_name.1 == "image/png" {
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            let cache = data_root
                .join("cli")
                .join("image-cache")
                .join(session_id)
                .join(format!("image-{:x}.png", hasher.finalize()));
            if let Some(parent) = cache.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| AttachmentError::Io(error.to_string()))?;
            }
            std::fs::write(&cache, &bytes)
                .map_err(|error| AttachmentError::Io(error.to_string()))?;
        }
        let stored = StoredAttachment {
            session_id: session_id.to_string(),
            path: target,
            media_type: file_name.1,
            total_bytes: bytes.len() as u64,
            file_name: file_name.0,
        };
        {
            let mut uploads = self
                .uploads
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            uploads.insert(
                upload_id.to_string(),
                Upload::Committed {
                    session_id: session_id.to_string(),
                    attachment_ref: attachment_ref.clone(),
                },
            );
        }
        self.refs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(attachment_ref.clone(), stored.clone());
        Ok((attachment_ref, stored))
    }

    /// abort：移除 staging（幂等；已 commit 的 ref 保留）。
    pub fn abort(&self, upload_id: &str, session_id: &str) -> Result<(), AttachmentError> {
        let mut uploads = self
            .uploads
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match uploads.get(upload_id) {
            Some(Upload::Staging {
                session_id: owner, ..
            }) if owner == session_id => {
                uploads.remove(upload_id);
                Ok(())
            }
            Some(Upload::Committed { .. }) => Ok(()),
            Some(_) => Err(AttachmentError::SessionMismatch),
            None => Err(AttachmentError::UploadNotFound),
        }
    }

    /// ref 授权判定（read/previewSource 与 conversation share 面共用）。
    pub fn authorize(&self, session_id: &str, attachment_ref: &str) -> RefAuthorization {
        let refs = self
            .refs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match refs.get(attachment_ref) {
            Some(stored) if stored.session_id == session_id => {
                RefAuthorization::Authorized(stored.clone())
            }
            Some(stored) => RefAuthorization::Foreign(stored.clone()),
            None => RefAuthorization::Unknown,
        }
    }

    /// 分段读（授权方自行判定）。
    pub fn read(
        &self,
        stored: &StoredAttachment,
        offset: u64,
        limit: u64,
    ) -> Result<(String, Option<u64>), AttachmentError> {
        use base64::engine::general_purpose::STANDARD as BASE64;
        let bytes =
            std::fs::read(&stored.path).map_err(|error| AttachmentError::Io(error.to_string()))?;
        let start = (offset as usize).min(bytes.len());
        let end = ((offset + limit) as usize).min(bytes.len());
        let slice = &bytes[start..end];
        let next_offset = if end < bytes.len() {
            Some(end as u64)
        } else {
            None
        };
        Ok((BASE64.encode(slice), next_offset))
    }
}

/// begin 的两支结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BeginOutcome {
    Staging,
    Committed(String),
}

fn artifact_path(
    data_root: &std::path::Path,
    session_id: &str,
    upload_id: &str,
    file_name: &str,
) -> PathBuf {
    let extension = std::path::Path::new(file_name)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{value}"))
        .unwrap_or_default();
    data_root
        .join("cli")
        .join("artifacts")
        .join(session_id)
        .join(format!(
            "prompt-attachment-upload-{upload_id}-tool-result-{}{extension}",
            uuid::Uuid::new_v4()
        ))
}
