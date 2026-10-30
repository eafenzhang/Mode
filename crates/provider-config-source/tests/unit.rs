//! 只读源的公开 API 集成测试：恢复、回落、sourceKey 与组合 revision。

use std::path::{Path, PathBuf};

use provider_config_source::{
    empty_personal_layer, read_mode_builtin_layer, read_personal_layer, read_snapshot,
    source_key_with_cwd,
};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pcs-unit-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, name: &str, content: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path.to_string_lossy().into_owned()
}

fn release(revision: i64) -> String {
    format!(
        r#"{{"schemaVersion":1,"revision":{revision},"config":{{"providerConfigRules":{{"templateRules":[],"providerRules":[{{"providerId":"builtin:p1","config":{{"group":"zai-family"}}}}]}},"modelConfigRules":{{"modelRules":[],"modelApiRules":[],"providerSiteRules":[],"templateModelRules":[],"builtinProviderModelRules":[]}}}}}}"#
    )
}

fn valid_personal() -> String {
    r#"{"schemaVersion":1,"config":{"providerOrder":["p1"],"providerConfigRules":{"providerRules":[{"providerId":"p1","providerName":"P","config":{"group":"standard-personal"}}]},"modelConfigRules":{"providerModelRules":[],"manualProviderModelRules":[]},"defaultModelSelection":{"providerId":"p1","modelId":"m1"}}}"#.to_owned()
}

#[test]
fn personal_recovery_is_empty_layer() {
    let dir = temp_dir("personal-recovery");
    let missing = dir.join("missing.json");
    assert_eq!(read_personal_layer(&missing), empty_personal_layer());

    let corrupt = write(&dir, "corrupt.json", "{oops");
    assert_eq!(read_personal_layer(&corrupt), empty_personal_layer());

    let unknown = write(
        &dir,
        "unknown.json",
        r#"{"schemaVersion":1,"config":{"providerConfigRules":{"providerRules":[]},"modelConfigRules":{"providerModelRules":[],"manualProviderModelRules":[]},"x":1}}"#,
    );
    assert_eq!(read_personal_layer(&unknown), empty_personal_layer());

    let valid = write(&dir, "valid.json", &valid_personal());
    let layer = read_personal_layer(&valid);
    assert_ne!(layer.revision, empty_personal_layer().revision);
    assert_eq!(layer.provider_order, Some(vec!["p1".to_owned()]));
    assert_eq!(
        layer.default_model_selection.unwrap().provider_id,
        "p1".to_owned()
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn builtin_missing_active_falls_back_and_source_key_uses_active_path() {
    let dir = temp_dir("builtin-fallback");
    let bundled = write(&dir, "bundled.json", &release(5));
    let active_literal = format!("{}/sub/../sub/active.json", dir.to_string_lossy());
    let layer = read_mode_builtin_layer(&bundled, Some(&active_literal)).unwrap();
    let expected_key = source_key_with_cwd(
        &format!("{}/sub/active.json", dir.to_string_lossy()),
        &std::env::current_dir().unwrap().to_string_lossy(),
    );
    assert_eq!(layer.revision, format!("mode-builtin:5:{expected_key}"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn snapshot_composes_layer_revisions() {
    let dir = temp_dir("snapshot-compose");
    let bundled = write(&dir, "bundled.json", &release(2));
    let personal_path = write(&dir, "provider_config.json", &valid_personal());
    let snapshot = read_snapshot(&bundled, &personal_path, None).unwrap();
    let expected = serde_json::to_string(&[
        snapshot.mode_builtin_revision.clone(),
        snapshot.personal_revision.clone(),
    ])
    .unwrap();
    assert_eq!(snapshot.revision, expected);
    assert_eq!(snapshot.personal_provider_order, vec!["p1".to_owned()]);
    std::fs::remove_dir_all(&dir).ok();
}
