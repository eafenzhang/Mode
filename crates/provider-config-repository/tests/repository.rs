use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use provider_config_file_codec::{encode_provider_config_file, ProviderConfigLayerUpdate};
use provider_config_repository::runtime_paths::{
    create_node_provider_runtime_path_env, resolve_node_provider_runtime_paths,
    ProviderRuntimePaths, MODE_BUILTIN_PROVIDER_CONFIG_FILE_ENV,
    MODE_PERSONAL_PROVIDER_CONFIG_FILE_ENV,
};
use provider_config_repository::{
    PersonalConfigRepository, PersonalConfigRepositoryOptions, RepositoryError,
};
use provider_config_source::empty_personal_layer;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pcr-test-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn empty_update() -> ProviderConfigLayerUpdate {
    let layer = empty_personal_layer();
    ProviderConfigLayerUpdate {
        providers: layer.providers,
        models: layer.models,
        provider_order: layer.provider_order,
        default_model_selection: None,
    }
}

fn repository(path: &Path) -> PersonalConfigRepository {
    PersonalConfigRepository::new(PersonalConfigRepositoryOptions {
        file_path: path.to_path_buf(),
        import_legacy: None,
        on_recovery: None,
    })
    .expect("repository")
}

#[test]
fn missing_file_reads_empty_without_creating_it() {
    let dir = temp_dir();
    let path = dir.join("provider_config.json");
    let repo = repository(&path);

    let layer = repo.read();
    assert_eq!(layer, empty_personal_layer());
    assert!(!path.exists());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn invalid_file_degrades_and_is_preserved() {
    let dir = temp_dir();
    let path = dir.join("provider_config.json");
    let original = "{not json";
    std::fs::write(&path, original).unwrap();
    let recovered = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = Arc::clone(&recovered);
    let repo = PersonalConfigRepository::new(PersonalConfigRepositoryOptions {
        file_path: path.clone(),
        import_legacy: None,
        on_recovery: Some(Arc::new(move |error: &RepositoryError| {
            sink.lock().unwrap().push(error.to_string());
        })),
    })
    .expect("repository");

    assert_eq!(repo.read(), empty_personal_layer());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    assert_eq!(recovered.lock().unwrap().len(), 1);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn non_canonical_file_is_rewritten_pretty() {
    let dir = temp_dir();
    let path = dir.join("provider_config.json");
    let loose = r#"{"schemaVersion":1,"config":{"providerConfigRules":{"providerRules":[]},"modelConfigRules":{"providerModelRules":[],"manualProviderModelRules":[]},"defaultModelSelection":{"providerId":" p1 ","modelId":" m1 "}}}"#;
    std::fs::write(&path, loose).unwrap();
    let repo = repository(&path);

    let layer = repo.read();
    let decoded = provider_config_file_codec::decode_provider_config_file(
        &provider_config::JsonValue::from_json_str(loose).unwrap(),
    )
    .unwrap();
    let written = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        written,
        serde_json::to_string_pretty(&encode_provider_config_file(&decoded)).expect("pretty json")
    );
    let selection = layer.default_model_selection.expect("trimmed selection");
    assert_eq!(selection.provider_id, "p1");
    assert_eq!(selection.model_id, "m1");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn legacy_import_writes_missing_file() {
    let dir = temp_dir();
    let path = dir.join("provider_config.json");
    let import = Arc::new(|| {
        Some(ProviderConfigLayerUpdate {
            provider_order: Some(vec!["alpha".to_string()]),
            ..empty_update()
        })
    });
    let repo = PersonalConfigRepository::new(PersonalConfigRepositoryOptions {
        file_path: path.clone(),
        import_legacy: Some(import),
        on_recovery: None,
    })
    .expect("repository");

    let layer = repo.read();
    assert_eq!(layer.provider_order, Some(vec!["alpha".to_string()]));
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(written.contains("\"alpha\""));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn update_writes_canonical_document() {
    let dir = temp_dir();
    let path = dir.join("provider_config.json");
    let repo = repository(&path);

    let updated = repo.update(|_| empty_update()).expect("update");
    assert_eq!(updated, empty_personal_layer());
    let written = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        written,
        serde_json::to_string_pretty(&encode_provider_config_file(&empty_update())).unwrap()
    );
    assert_eq!(repo.read(), updated);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn update_on_invalid_file_errors_and_preserves_it() {
    let dir = temp_dir();
    let path = dir.join("provider_config.json");
    let original = "{broken";
    std::fs::write(&path, original).unwrap();
    let repo = repository(&path);

    let error = repo.update(|_| empty_update()).unwrap_err();
    assert!(matches!(error, RepositoryError::Invalid(_)));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn empty_file_path_is_rejected() {
    let error = match PersonalConfigRepository::new(PersonalConfigRepositoryOptions {
        file_path: PathBuf::from("  "),
        import_legacy: None,
        on_recovery: None,
    }) {
        Ok(_) => panic!("空 filePath 必须拒绝"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("filePath"));
}

#[test]
fn runtime_paths_require_both_or_neither() {
    let empty = BTreeMap::new();
    assert_eq!(resolve_node_provider_runtime_paths(&empty).unwrap(), None);

    let only_builtin = BTreeMap::from([(
        MODE_BUILTIN_PROVIDER_CONFIG_FILE_ENV.to_string(),
        "/tmp/builtin.json".to_string(),
    )]);
    assert!(resolve_node_provider_runtime_paths(&only_builtin).is_err());

    let mut both = BTreeMap::new();
    both.insert(
        MODE_BUILTIN_PROVIDER_CONFIG_FILE_ENV.to_string(),
        "  /tmp/builtin.json  ".to_string(),
    );
    both.insert(
        MODE_PERSONAL_PROVIDER_CONFIG_FILE_ENV.to_string(),
        "/tmp/personal.json".to_string(),
    );
    let paths = resolve_node_provider_runtime_paths(&both).unwrap().unwrap();
    assert_eq!(
        paths,
        ProviderRuntimePaths {
            mode_builtin_file_path: "/tmp/builtin.json".to_string(),
            personal_file_path: "/tmp/personal.json".to_string(),
        }
    );
    let env = create_node_provider_runtime_path_env(&paths);
    assert_eq!(
        env.get(MODE_BUILTIN_PROVIDER_CONFIG_FILE_ENV),
        Some(&"/tmp/builtin.json".to_string())
    );
    assert_eq!(
        env.get(MODE_PERSONAL_PROVIDER_CONFIG_FILE_ENV),
        Some(&"/tmp/personal.json".to_string())
    );
}
