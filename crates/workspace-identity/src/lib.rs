//! workspace 身份归一与 hash（W1 基元）。
//!
//! 真源：
//! - `packages/shared/src/remote-workspace-identity.ts::normalizeWorkspacePathForIdentity`
//! - `packages/services/src/paths.ts::getWorkspaceKey` / `getWorkspaceHash`
//!
//! 口径（AGENTS.md「Workspace Identity」）：身份 key 统一为
//! `workspaceIdentity?.trim() || workspacePath`，用于去重、绑定、缓存、队列、持久化与请求关联；
//! **实际文件操作、命令 cwd、Git 与路径展示仍用 `workspacePath`**，不要拿 hash 当路径。

use sha2::{Digest, Sha256};

/// 与 TS 一致：反斜杠转 `/`、合并重复 `/`、去掉首尾 `/`；
/// Windows 盘符保留 `X:/` 形态，其余路径统一补一个前导 `/`（所以空串归一为 `/`）。
pub fn normalize_workspace_path_for_identity(path: &str) -> String {
    let unified = path.replace('\\', "/");
    let mut normalized = String::with_capacity(unified.len());
    let mut previous_slash = false;
    for ch in unified.chars() {
        if ch == '/' {
            if previous_slash {
                continue;
            }
            previous_slash = true;
        } else {
            previous_slash = false;
        }
        normalized.push(ch);
    }
    let trimmed = normalized.trim_matches('/');
    let is_windows_drive = {
        let bytes = trimmed.as_bytes();
        bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
            && (bytes.len() == 2 || bytes[2] == b'/')
    };
    if is_windows_drive {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    }
}

/// 身份 key：identity 去空白后非空就用它，否则用路径。
pub fn workspace_key(workspace_path: &str, workspace_identity: Option<&str>) -> String {
    match workspace_identity.map(str::trim) {
        Some(identity) if !identity.is_empty() => identity.to_string(),
        _ => workspace_path.to_string(),
    }
}

/// `workspaceKey` 的 SHA-256 前 12 位十六进制（与 Mode session 持久化一致）。
pub fn workspace_hash(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    hex[..12].to_string()
}

/// 便捷组合：先取 key 再取 hash。
pub fn workspace_hash_for(workspace_path: &str, workspace_identity: Option<&str>) -> String {
    workspace_hash(&workspace_key(workspace_path, workspace_identity))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_backslashes_and_duplicate_slashes() {
        assert_eq!(
            normalize_workspace_path_for_identity("D:\\\\ZCodium\\\\"),
            "D:/ZCodium"
        );
        assert_eq!(
            normalize_workspace_path_for_identity("/home//user/"),
            "/home/user"
        );
        assert_eq!(normalize_workspace_path_for_identity("relative/path"), "/relative/path");
        assert_eq!(normalize_workspace_path_for_identity(""), "/");
        assert_eq!(normalize_workspace_path_for_identity("//D:/x"), "D:/x");
        assert_eq!(normalize_workspace_path_for_identity("C:"), "C:");
    }

    #[test]
    fn identity_wins_over_path_and_is_trimmed() {
        assert_eq!(workspace_key("D:/w", Some(" remote:lan:1:D:/w ")), "remote:lan:1:D:/w");
        assert_eq!(workspace_key("D:/w", Some("   ")), "D:/w");
        assert_eq!(workspace_key("D:/w", None), "D:/w");
    }

    #[test]
    fn hash_matches_mode_session_convention() {
        // 期望值由 Node `createHash("sha256").update(key).digest("hex").slice(0,12)` 生成。
        assert_eq!(workspace_hash("D:/ZCodium"), "67dc4aec24f7");
        assert_eq!(workspace_hash("/home/user/project"), "9dad1e4e08b0");
        assert_eq!(workspace_hash("/"), "8a5edab28263");
        assert_eq!(
            workspace_hash("remote:lan:100.90.96.50:45880:D:/ZCodium"),
            "e2bfc498d523"
        );
        assert_eq!(
            workspace_hash_for("D:/ignored", Some("D:/ZCodium")),
            "67dc4aec24f7"
        );
    }
}
