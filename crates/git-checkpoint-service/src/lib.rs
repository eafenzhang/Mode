//! Git workspace checkpoint 服务（`gitCheckpointService.ts`）。
//!
//! 真源：`packages/services/src/git/gitCheckpointService.ts` 与 `repo/gitCheckpoint*.ts`。
//! 行为契约与已知差异见 `docs/specs/git-checkpoint-service.md`。
//!
//! 状态所有权：
//! - manifest（`<dataRoot>/v2/checkpoints/<workspaceHash>/<id>.json`）由本 crate 独占；
//! - Git 侧状态（隐藏 ref、隐藏 commit）由 Git 仓库独占，服务只写 `refs/mode/checkpoints/...`；
//! - 临时 index 位于 `<dataRoot>/git-checkpoint-index/`，单次操作结束即删除。

mod repo;
mod store;
mod types;

use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use repo::GitCheckpointRepo;
pub use store::GitCheckpointStore;
pub use types::{
    GitCheckpointConflict, GitCheckpointDiff, GitCheckpointFileDiff, GitCheckpointMeta,
    GitCheckpointRestoreResult,
};

/// 与 TS `createGitCheckpointService` 对齐的门面：加载 + 校验 + 委托 repo/store。
pub struct GitCheckpointService {
    store: GitCheckpointStore,
    repo: GitCheckpointRepo,
}

impl GitCheckpointService {
    /// 默认布局：manifest 在 `<data_root>/v2/checkpoints`，临时 index 在
    /// `<data_root>/git-checkpoint-index`（与 `getAppConfigDir()` / `getGitCheckpointIndexRootDir()` 一致）。
    pub fn new(provider: Arc<git_cli::GitCommandProvider>, data_root: &Path) -> Self {
        let store = GitCheckpointStore::new(data_root.join("v2").join("checkpoints"));
        let repo = GitCheckpointRepo::new(
            Arc::clone(&provider),
            git_service::GitCliRepo::new(provider),
            data_root.join("git-checkpoint-index"),
        );
        Self { store, repo }
    }

    pub fn with_parts(store: GitCheckpointStore, repo: GitCheckpointRepo) -> Self {
        Self { store, repo }
    }

    fn load_checkpoint(
        &self,
        workspace_path: &str,
        checkpoint_id: &str,
    ) -> Result<GitCheckpointMeta, String> {
        let meta = self
            .store
            .load(workspace_path, checkpoint_id)
            .ok_or_else(|| format!("Checkpoint does not exist: {checkpoint_id}"))?;
        if meta.workspace_path != workspace_path {
            return Err(format!("Checkpoint workspace mismatch: {checkpoint_id}"));
        }
        Ok(meta)
    }

    pub fn create_checkpoint(&self, workspace_path: &Path) -> Result<GitCheckpointMeta, String> {
        let checkpoint_id = uuid::Uuid::new_v4().to_string();
        let meta = self
            .repo
            .create_checkpoint(workspace_path, &checkpoint_id)?;
        self.store.save(&meta)?;
        Ok(meta)
    }

    pub fn diff_checkpoints(
        &self,
        workspace_path: &Path,
        from_checkpoint_id: &str,
        to_checkpoint_id: &str,
    ) -> Result<GitCheckpointDiff, String> {
        let workspace_text = workspace_path.to_string_lossy().into_owned();
        let from = self.load_checkpoint(&workspace_text, from_checkpoint_id)?;
        let to = self.load_checkpoint(&workspace_text, to_checkpoint_id)?;
        ensure_comparable(&from, &to)?;
        self.repo.diff_checkpoints(workspace_path, &from, &to)
    }

    pub fn restore_between_checkpoints(
        &self,
        workspace_path: &Path,
        from_checkpoint_id: &str,
        to_checkpoint_id: &str,
        force: bool,
    ) -> Result<GitCheckpointRestoreResult, String> {
        let workspace_text = workspace_path.to_string_lossy().into_owned();
        let from = self.load_checkpoint(&workspace_text, from_checkpoint_id)?;
        let to = self.load_checkpoint(&workspace_text, to_checkpoint_id)?;
        ensure_comparable(&from, &to)?;
        self.repo
            .restore_between_checkpoints(workspace_path, &from, &to, force)
    }

    pub fn delete_checkpoint(
        &self,
        workspace_path: &Path,
        checkpoint_id: &str,
    ) -> Result<(), String> {
        let workspace_text = workspace_path.to_string_lossy().into_owned();
        let checkpoint = self.load_checkpoint(&workspace_text, checkpoint_id)?;
        self.repo.delete_checkpoint(workspace_path, &checkpoint)?;
        self.store.delete(&workspace_text, checkpoint_id)
    }

    /// 数据根（测试与诊断用）。
    pub fn checkpoints_root_dir(&self) -> &Path {
        self.store.root_dir()
    }
}

fn ensure_comparable(left: &GitCheckpointMeta, right: &GitCheckpointMeta) -> Result<(), String> {
    if left.repo_root != right.repo_root {
        return Err("Checkpoint repoRoot mismatch.".to_string());
    }
    if left.scope != right.scope {
        return Err("Checkpoint scope mismatch.".to_string());
    }
    if left.workspace_in_repo_path != right.workspace_in_repo_path {
        return Err("Checkpoint workspace scope mismatch.".to_string());
    }
    Ok(())
}

/// 默认数据根下的服务（in-process 频道用）。
pub fn default_service(data_root: PathBuf) -> Arc<GitCheckpointService> {
    let provider = Arc::new(git_cli::GitCommandProvider::new(Arc::new(
        git_cli::GitEnvironmentProvider::new(),
    )));
    Arc::new(GitCheckpointService::new(provider, &data_root))
}
