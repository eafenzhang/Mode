//! Checkpoint manifest 存储（`gitCheckpointStore.ts`）。
//!
//! 布局：`<root>/<workspaceHash>/<checkpointId>.json`；严格校验字段，损坏文件按缺失处理；
//! 写临时文件 + rename（同步实现，写操作由服务层互斥锁串行化）。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::types::GitCheckpointMeta;

#[derive(Debug)]
pub struct GitCheckpointStore {
    root_dir: PathBuf,
    write_lock: Mutex<()>,
}

impl GitCheckpointStore {
    pub fn new(root_dir: PathBuf) -> Self {
        Self {
            root_dir,
            write_lock: Mutex::new(()),
        }
    }

    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    fn checkpoint_dir(&self, workspace_path: &str) -> PathBuf {
        self.root_dir
            .join(workspace_identity::workspace_hash_for(workspace_path, None))
    }

    fn checkpoint_file_path(&self, workspace_path: &str, checkpoint_id: &str) -> PathBuf {
        self.checkpoint_dir(workspace_path)
            .join(format!("{checkpoint_id}.json"))
    }

    pub fn save(&self, meta: &GitCheckpointMeta) -> Result<(), String> {
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let file_path = self.checkpoint_file_path(&meta.workspace_path, &meta.checkpoint_id);
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let text = format!(
            "{}\n",
            serde_json::to_string_pretty(&meta).map_err(|error| error.to_string())?
        );
        let temp_path = file_path.with_extension(format!(
            "{}.tmp",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::write(&temp_path, text).map_err(|error| error.to_string())?;
        std::fs::rename(&temp_path, &file_path).map_err(|error| {
            let _ = std::fs::remove_file(&temp_path);
            error.to_string()
        })
    }

    pub fn load(&self, workspace_path: &str, checkpoint_id: &str) -> Option<GitCheckpointMeta> {
        let file_path = self.checkpoint_file_path(workspace_path, checkpoint_id);
        let raw = std::fs::read_to_string(file_path).ok()?;
        let parsed: serde_json::Value = serde_json::from_str(&raw).ok()?;
        parse_meta(&parsed)
    }

    pub fn delete(&self, workspace_path: &str, checkpoint_id: &str) -> Result<(), String> {
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let file_path = self.checkpoint_file_path(workspace_path, checkpoint_id);
        let _ = std::fs::remove_file(file_path);
        Ok(())
    }
}

/// `isGitCheckpointMeta`：字段类型严格，`scope` 必须是 `workspace`。
fn parse_meta(value: &serde_json::Value) -> Option<GitCheckpointMeta> {
    let object = value.as_object()?;
    let text = |key: &str| object.get(key).and_then(serde_json::Value::as_str);
    let number = |key: &str| object.get(key).and_then(serde_json::Value::as_u64);
    if object.get("scope").and_then(serde_json::Value::as_str) != Some("workspace") {
        return None;
    }
    Some(GitCheckpointMeta {
        checkpoint_id: text("checkpointId")?.to_string(),
        workspace_path: text("workspacePath")?.to_string(),
        repo_root: text("repoRoot")?.to_string(),
        workspace_in_repo_path: text("workspaceInRepoPath")?.to_string(),
        created_at: number("createdAt")?,
        ref_name: text("refName")?.to_string(),
        commit_oid: text("commitOid")?.to_string(),
        scope: "workspace".to_string(),
    })
}
