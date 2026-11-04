//! Mode Built-in Remote Release 落盘（`applyRemoteRelease`）。
//!
//! 真源：`packages/provider-node/src/mode-builtin-provider-config-source.ts`。
//! 在 Active 路径的跨进程锁内重新选候选并物化，再按 revision 决定
//! stale/unchanged/updated；同 revision 不同内容视为错误，不能静默覆盖。
//! 文件监听、`observedSignature` 与 `remote-updated` 通知留在宿主。

use std::io::ErrorKind;
use std::path::Path;

use atomic_file::{acquire_file_lock, write_private_text_atomic, WriteOptions};
use provider_config::JsonValue;
use provider_config_source::{
    decode_mode_builtin_release, select_mode_builtin_release, serialize_mode_builtin_release,
    ModeBuiltinRelease,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyRemoteReleaseOutcome {
    Updated,
    Unchanged,
    Stale,
}

/// 解锁前按 TS `applyRemoteRelease` 的锁内顺序执行：
/// 1. 读取 Bundled/Active 并按候选规则选中当前值，必要时把选中值原子写回 Active；
/// 2. `release.revision < current.revision` → `Stale`；
/// 3. revision 相同：内容相同 → `Unchanged`，否则报错；
/// 4. revision 更大 → 私有原子写 Active → `Updated`。
pub fn apply_remote_release(
    bundled_path: &Path,
    active_path: &Path,
    release: &ModeBuiltinRelease,
) -> Result<ApplyRemoteReleaseOutcome, String> {
    if let Some(parent) = active_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let bundled = bundled_path.to_string_lossy().into_owned();
    let active = active_path.to_string_lossy().into_owned();
    let _lock = acquire_file_lock(active_path, &WriteOptions::default())
        .map_err(|error| error.to_string())?;

    let current =
        select_mode_builtin_release(&bundled, Some(&active)).map_err(|error| error.to_string())?;

    // 与 TS `#readAndMaterializeLocked` 一致：Active 缺失、无效或落后时先物化，
    // 让后续 revision 比较落在已落盘的 Active 上。
    if active != bundled {
        let active_signature = read_active_release(active_path)?.map(|value| signature(&value));
        if active_signature.as_deref() != Some(signature(&current).as_str()) {
            write_active(active_path, &current)?;
        }
    }

    if release.revision < current.revision {
        return Ok(ApplyRemoteReleaseOutcome::Stale);
    }
    if release.revision == current.revision {
        if serialize_mode_builtin_release(release) == serialize_mode_builtin_release(&current) {
            return Ok(ApplyRemoteReleaseOutcome::Unchanged);
        }
        return Err(format!(
            "Mode Built-in 相同 revision {} 对应不同内容",
            release.revision
        ));
    }
    write_active(active_path, release)?;
    Ok(ApplyRemoteReleaseOutcome::Updated)
}

/// TS `signatureOf`：revision + 规范序列化。
fn signature(release: &ModeBuiltinRelease) -> String {
    format!(
        "{}:{}",
        release.revision,
        serialize_mode_builtin_release(release)
    )
}

/// Active 只是可丢弃缓存：读取/解析失败都按“无候选”处理。
fn read_active_release(path: &Path) -> Result<Option<ModeBuiltinRelease>, String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let text = String::from_utf8_lossy(&bytes);
    let Ok(value) = JsonValue::from_json_str(&text) else {
        return Ok(None);
    };
    Ok(decode_mode_builtin_release(&value).ok())
}

fn write_active(path: &Path, release: &ModeBuiltinRelease) -> Result<(), String> {
    // TS: JSON.stringify(encodeModeBuiltinRelease(release), null, 2)，无尾随换行。
    let content = serde_json::to_string_pretty(&release.to_json()).map_err(|error| error.to_string())?;
    // 外层已持锁；原子写内部再抢锁会自锁超时。
    let options = WriteOptions {
        use_file_lock: false,
        ..WriteOptions::default()
    };
    write_private_text_atomic(path, &content, &options).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release_json(revision: i64, extra_model_rules: &str) -> String {
        format!(
            r#"{{"schemaVersion":1,"revision":{revision},"config":{{"providerConfigRules":{{"templateRules":[],"providerRules":[{{"providerId":"p1","config":{{"group":"zai-family"}}}}]}},"modelConfigRules":{{"modelRules":{extra_model_rules},"modelApiRules":[],"providerSiteRules":[],"templateModelRules":[],"builtinProviderModelRules":[]}}}}}}"#
        )
    }

    fn release(revision: i64) -> ModeBuiltinRelease {
        release_with(revision, "[]")
    }

    fn release_with(revision: i64, extra_model_rules: &str) -> ModeBuiltinRelease {
        let value = JsonValue::from_json_str(&release_json(revision, extra_model_rules)).unwrap();
        decode_mode_builtin_release(&value).unwrap()
    }

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("pcr-apply-{name}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn bundled(&self, release: &ModeBuiltinRelease) -> std::path::PathBuf {
            let path = self.0.join("bundled.json");
            std::fs::write(&path, release.serialize()).unwrap();
            path
        }

        fn active(&self) -> std::path::PathBuf {
            self.0.join("active").join("active.json")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn newer_release_updates_active_with_pretty_json() {
        let dir = TempDir::new("newer");
        let bundled = dir.bundled(&release(5));
        let active = dir.active();
        let outcome = apply_remote_release(&bundled, &active, &release(7)).unwrap();
        assert_eq!(outcome, ApplyRemoteReleaseOutcome::Updated);
        let content = std::fs::read_to_string(&active).unwrap();
        assert!(content.contains("\n  \"schemaVersion\": 1,"), "{content}");
        assert!(content.contains("\n  \"revision\": 7,"), "{content}");
        assert!(!content.ends_with('\n'));
    }

    #[test]
    fn older_release_is_stale_and_active_stays_untouched() {
        let dir = TempDir::new("stale");
        let bundled = dir.bundled(&release(5));
        let active = dir.active();
        apply_remote_release(&bundled, &active, &release(7)).unwrap();
        let before = std::fs::read_to_string(&active).unwrap();
        let outcome = apply_remote_release(&bundled, &active, &release(5)).unwrap();
        assert_eq!(outcome, ApplyRemoteReleaseOutcome::Stale);
        assert_eq!(std::fs::read_to_string(&active).unwrap(), before);
    }

    #[test]
    fn same_revision_same_content_is_unchanged() {
        let dir = TempDir::new("unchanged");
        let bundled = dir.bundled(&release(5));
        let active = dir.active();
        let outcome = apply_remote_release(&bundled, &active, &release(5)).unwrap();
        assert_eq!(outcome, ApplyRemoteReleaseOutcome::Unchanged);
        assert!(active.exists());
    }

    #[test]
    fn same_revision_different_content_is_rejected() {
        let dir = TempDir::new("conflict");
        let bundled = dir.bundled(&release(5));
        let active = dir.active();
        apply_remote_release(&bundled, &active, &release(5)).unwrap();
        let conflicting = release_with(5, r#"[{"modelMatch":"x-.*","config":{"enabled":true}}]"#);
        let error = apply_remote_release(&bundled, &active, &conflicting).unwrap_err();
        assert_eq!(error, "Mode Built-in 相同 revision 5 对应不同内容");
    }

    #[test]
    fn missing_active_and_parent_are_materialized_before_applying() {
        let dir = TempDir::new("missing-active");
        let bundled = dir.bundled(&release(5));
        let active = dir.active();
        assert!(!active.exists());
        let outcome = apply_remote_release(&bundled, &active, &release(7)).unwrap();
        assert_eq!(outcome, ApplyRemoteReleaseOutcome::Updated);
        let value = JsonValue::from_json_str(&std::fs::read_to_string(&active).unwrap()).unwrap();
        assert_eq!(decode_mode_builtin_release(&value).unwrap().revision, 7);
    }

    #[test]
    fn invalid_active_is_replaced_by_selected_release() {
        let dir = TempDir::new("invalid-active");
        let bundled = dir.bundled(&release(5));
        let active = dir.active();
        std::fs::create_dir_all(active.parent().unwrap()).unwrap();
        std::fs::write(&active, "{not json").unwrap();
        let outcome = apply_remote_release(&bundled, &active, &release(5)).unwrap();
        assert_eq!(outcome, ApplyRemoteReleaseOutcome::Unchanged);
        let value = JsonValue::from_json_str(&std::fs::read_to_string(&active).unwrap()).unwrap();
        assert_eq!(decode_mode_builtin_release(&value).unwrap().revision, 5);
    }
}
