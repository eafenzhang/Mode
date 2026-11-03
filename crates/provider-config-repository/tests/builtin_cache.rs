use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use provider_config_repository::cache_paths::{
    create_mode_builtin_endpoint_key, normalize_mode_builtin_endpoint_origin,
    resolve_mode_builtin_cache_paths, resolve_mode_builtin_client_platform,
    ModeBuiltinCachePathOptions,
};
use provider_config_repository::materializer::{
    materialize_mode_builtin_provider_config, MaterializeModeBuiltinProviderConfigOptions,
};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pcb-test-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn release_json() -> &'static str {
    r#"{"schemaVersion":1,"revision":3,"config":{"providerConfigRules":{"templateRules":[],"providerRules":[]},"modelConfigRules":{"modelRules":[],"modelApiRules":[],"providerSiteRules":[],"templateModelRules":[],"builtinProviderModelRules":[]}}}"#
}

#[test]
fn platform_mapping_matches_process_platform_and_arch() {
    assert_eq!(
        resolve_mode_builtin_client_platform("win32", "x64"),
        "windows-x86_64"
    );
    assert_eq!(
        resolve_mode_builtin_client_platform("darwin", "arm64"),
        "darwin-aarch64"
    );
    assert_eq!(
        resolve_mode_builtin_client_platform("linux", "ia32"),
        "linux-ia32"
    );
    assert_eq!(
        resolve_mode_builtin_client_platform("freebsd", "arm"),
        "freebsd-arm"
    );
}

#[test]
fn endpoint_key_normalizes_origin() {
    let canonical = create_mode_builtin_endpoint_key("https://example.com").unwrap();
    assert_eq!(
        canonical,
        create_mode_builtin_endpoint_key("  https://Example.COM:443/path?q=1  ").unwrap()
    );
    assert_eq!(canonical.len(), "endpoint-".len() + 32);
    assert!(canonical.starts_with("endpoint-"));
    assert_ne!(
        canonical,
        create_mode_builtin_endpoint_key("http://example.com").unwrap()
    );

    assert!(create_mode_builtin_endpoint_key("").is_err());
    assert!(create_mode_builtin_endpoint_key("ftp://example.com").is_err());
    assert_eq!(
        normalize_mode_builtin_endpoint_origin("https://example.com/a").unwrap(),
        "https://example.com"
    );
}

#[test]
fn cache_paths_isolate_platform_version_and_endpoint() {
    let root = temp_dir();
    let options = ModeBuiltinCachePathOptions {
        environment_config_root: root.clone(),
        platform: "windows".to_string(),
        app_version: "0.0.1".to_string(),
        mode_endpoint_origin: "https://example.com".to_string(),
    };
    let paths = resolve_mode_builtin_cache_paths(&options).unwrap();
    let endpoint = create_mode_builtin_endpoint_key("https://example.com").unwrap();
    let directory = root
        .join("runtime")
        .join("provider")
        .join("windows")
        .join("0.0.1")
        .join(endpoint);
    assert_eq!(paths.active_file_path, directory.join("mode-builtin.json"));
    assert_eq!(
        paths.control_file_path,
        directory.join("mode-builtin-refresh.json")
    );

    let invalid = ModeBuiltinCachePathOptions {
        platform: "../escape".to_string(),
        ..options.clone()
    };
    assert!(resolve_mode_builtin_cache_paths(&invalid).is_err());
    let invalid_version = ModeBuiltinCachePathOptions {
        app_version: "a/b".to_string(),
        ..options
    };
    assert!(resolve_mode_builtin_cache_paths(&invalid_version).is_err());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn materializer_writes_normalized_bundled_release() {
    let root = temp_dir();
    let options = MaterializeModeBuiltinProviderConfigOptions {
        environment_config_root: root.clone(),
        content: release_json().to_string(),
    };
    let path = materialize_mode_builtin_provider_config(&options).unwrap();
    let expected = root
        .join("runtime")
        .join("provider")
        .join("bundled")
        .join("mode-builtin.json");
    assert_eq!(path, expected);
    let first = std::fs::read_to_string(&path).unwrap();
    assert!(first.ends_with('\n'), "物化文件应以换行结尾");

    materialize_mode_builtin_provider_config(&options).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), first);

    let invalid = MaterializeModeBuiltinProviderConfigOptions {
        environment_config_root: root.clone(),
        content: "{not json".to_string(),
    };
    assert!(materialize_mode_builtin_provider_config(&invalid).is_err());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn materializer_creates_nested_directories() {
    let root = temp_dir();
    let options = MaterializeModeBuiltinProviderConfigOptions {
        environment_config_root: root.join("nested").join("config"),
        content: release_json().to_string(),
    };
    let path = materialize_mode_builtin_provider_config(&options).unwrap();
    assert!(Path::new(&path).exists());
    std::fs::remove_dir_all(&root).ok();
}
