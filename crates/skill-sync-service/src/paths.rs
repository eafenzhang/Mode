//! 路径安全校验（`skillSyncPath.ts`）。

use std::path::{Path, PathBuf};

pub fn normalize_skill_sync_relative_path(
    path: &str,
    unsafe_label: &str,
) -> Result<String, String> {
    let normalized = path
        .replace('\\', "/")
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_string();
    let is_absolute = path.starts_with('/')
        || path.starts_with('\\')
        || (path.len() >= 3
            && path.as_bytes()[0].is_ascii_alphabetic()
            && path.as_bytes()[1] == b':');
    let has_bad_segment = normalized
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..");
    if normalized.is_empty() || is_absolute || path.contains('\\') || has_bad_segment {
        return Err(format!("{unsafe_label}: {path}"));
    }
    Ok(normalized)
}

pub fn resolve_skill_sync_path_within(
    target_root: &Path,
    path: &str,
    unsafe_label: &str,
) -> Result<PathBuf, String> {
    let normalized_root = absolute_normalized(target_root);
    let normalized = normalize_skill_sync_relative_path(path, unsafe_label)?;
    let target = normalized
        .split('/')
        .fold(normalized_root.clone(), |base, segment| base.join(segment));
    let relative = target
        .strip_prefix(&normalized_root)
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let escaped = relative
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir));
    if relative.as_os_str().is_empty() || escaped {
        return Err(format!("{unsafe_label}: {path}"));
    }
    Ok(target)
}

fn absolute_normalized(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    plugin_discovery::js::normalize_lexical(&absolute)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_paths() {
        assert!(normalize_skill_sync_relative_path("../x", "unsafe").is_err());
        assert!(normalize_skill_sync_relative_path("/abs", "unsafe").is_err());
        assert!(normalize_skill_sync_relative_path("C:/x", "unsafe").is_err());
        assert!(normalize_skill_sync_relative_path("a\\b", "unsafe").is_err());
        assert!(normalize_skill_sync_relative_path("a/./b", "unsafe").is_err());
        assert_eq!(
            normalize_skill_sync_relative_path("group/skill", "unsafe").unwrap(),
            "group/skill"
        );
    }

    #[test]
    fn resolve_stays_within_root() {
        let root = std::env::temp_dir().join("zc-skill-sync-root");
        assert!(resolve_skill_sync_path_within(&root, "a/b", "unsafe").is_ok());
        assert!(resolve_skill_sync_path_within(&root, "../a", "unsafe").is_err());
    }
}
