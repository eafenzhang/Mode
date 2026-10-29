//! 镜像快照的磁盘存储：
//! `{dataRoot}/v2/conversation-mirror/<workspaceHash>/<originDeviceId>.json`。
//!
//! 一个 origin 一个文件是幂等的关键：对端重复发布只覆盖自己的文件。读取端对单文件损坏
//! 只跳过该文件（fail-soft），不拖垮整个工作区。

use std::path::{Path, PathBuf};

use atomic_file::{write_text_atomic, AtomicError, WriteOptions};
use workspace_identity::normalize_workspace_path_for_identity;
use workspace_identity::workspace_hash;

use crate::model::{validate_snapshot, Snapshot};

const DIR_NAME: &str = "conversation-mirror";

pub struct ConversationMirrorStore {
    config_dir: PathBuf,
    options: WriteOptions,
}

impl ConversationMirrorStore {
    /// `data_root` 是 `{dataBaseDir}/.mode`；配置文件目录是它下面的 `v2`。
    pub fn new(data_root: impl Into<PathBuf>) -> Self {
        Self {
            config_dir: Path::new(&data_root.into()).join("v2"),
            options: WriteOptions::default(),
        }
    }

    pub fn workspace_dir(&self, workspace_path: &str) -> PathBuf {
        self.config_dir
            .join(DIR_NAME)
            .join(workspace_hash(&normalize_workspace_path_for_identity(
                workspace_path,
            )))
    }

    /// 读全部快照；目录缺失或不可读都返回空（镜像降级为空，不让调用方失败）。
    pub fn read_workspace_snapshots(&self, workspace_path: &str) -> Vec<Snapshot> {
        let dir = self.workspace_dir(workspace_path);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut snapshots = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".json") {
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
                continue;
            };
            if let Ok(snapshot) = validate_snapshot(&value) {
                snapshots.push(snapshot);
            }
        }
        snapshots
    }

    pub fn write_snapshot(&self, snapshot: &Snapshot) -> Result<(), AtomicError> {
        let file = self
            .workspace_dir(&snapshot.workspace_path)
            .join(format!("{}.json", file_stem(&snapshot.origin_device_id)));
        let text = serde_json::to_string(snapshot).unwrap_or_else(|_| "{}".to_string());
        write_text_atomic(&file, &text, &self.options)
    }
}

/// 与 TS `conversationMirrorSnapshotFileStem` 一致：非 `[A-Za-z0-9._-]` 一律换成 `_`。
fn file_stem(origin_device_id: &str) -> String {
    origin_device_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_origin_ids_for_file_names() {
        assert_eq!(file_stem("abc-123_ok.json"), "abc-123_ok.json");
        assert_eq!(file_stem("a/b:c d"), "a_b_c_d");
    }
}
