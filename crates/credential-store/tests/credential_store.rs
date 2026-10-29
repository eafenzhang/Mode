//! `CredentialStore` 的行为测试：对应 `docs/specs/credential-store.md` 的验收清单。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use credential_store::{CredentialError, CredentialStore, CREDENTIAL_DECRYPT_ERROR_CODE};

const SECRET: &str = "store-test-secret";

fn temp_dir(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "mode-credential-{}-{}-{name}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("创建临时目录");
    path
}

fn store(dir: &Path) -> CredentialStore {
    CredentialStore::with_secret(dir, SECRET)
}

fn count_backups(file: &Path) -> usize {
    let Some(parent) = file.parent() else {
        return 0;
    };
    let prefix = format!(
        "{}.corrupt-",
        file.file_name().unwrap().to_string_lossy()
    );
    fs::read_dir(parent)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .count()
}

#[test]
fn missing_file_reads_as_empty() {
    let dir = temp_dir("missing");
    let store = store(&dir);
    assert_eq!(store.load("anything").unwrap(), None);
    assert!(store.list("").unwrap().is_empty());
    assert!(!store.config_file().exists());
}

#[test]
fn save_then_load_round_trips() {
    let dir = temp_dir("roundtrip");
    let store = store(&dir);
    store.save("plain", "hello world").unwrap();
    store.save("unicode", "中文凭据 🔐").unwrap();
    store.save("spaces", "  keep surrounding spaces  ").unwrap();

    assert_eq!(store.load("plain").unwrap().as_deref(), Some("hello world"));
    assert_eq!(
        store.load("unicode").unwrap().as_deref(),
        Some("中文凭据 🔐")
    );
    assert_eq!(
        store.load("spaces").unwrap().as_deref(),
        Some("  keep surrounding spaces  ")
    );
    assert_eq!(store.load("missing").unwrap(), None);
}

#[test]
fn writes_encrypted_json_with_trailing_newline() {
    let dir = temp_dir("shape");
    let store = store(&dir);
    store.save("k", "v").unwrap();

    let text = fs::read_to_string(store.config_file()).unwrap();
    assert!(text.ends_with("\n"), "必须保留尾随换行");
    assert!(text.contains("\n  \"k\": \"enc:v1:"), "两空格缩进且值已加密");

    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(parsed["k"].as_str().unwrap().starts_with("enc:v1:"));
    assert!(!text.contains("\"v\""), "明文不得落盘");
}

#[cfg(unix)]
#[test]
fn credentials_file_is_private() {
    use std::os::unix::fs::PermissionsExt;

    let dir = temp_dir("mode");
    let store = store(&dir);
    store.save("k", "v").unwrap();

    let mode = fs::metadata(store.config_file())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "凭据文件必须是 0600");
}

#[test]
fn empty_key_is_rejected_after_trim() {
    let dir = temp_dir("empty-key");
    let store = store(&dir);
    assert!(matches!(
        store.save("   ", "v"),
        Err(CredentialError::KeyInvalid)
    ));
    assert!(matches!(
        store.load("\t\n"),
        Err(CredentialError::KeyInvalid)
    ));
    assert!(!store.config_file().exists(), "校验失败不得创建文件");
}

#[test]
fn keys_are_trimmed_on_both_sides() {
    let dir = temp_dir("trim");
    let store = store(&dir);
    store.save("  key  ", "value").unwrap();
    assert_eq!(store.load(" key ").unwrap().as_deref(), Some("value"));
    assert_eq!(store.list("").unwrap(), vec!["key".to_string()]);
}

#[test]
fn empty_value_reproduces_ts_format_error() {
    let dir = temp_dir("empty-value");
    let store = store(&dir);
    // TS 的加解密都允许写入空串，但读回时密文段为空会先撞上「密文格式非法」。
    store.save("k", "").unwrap();
    match store.load("k") {
        Err(CredentialError::Decrypt { reason }) => assert_eq!(reason, "密文格式非法"),
        other => panic!("应当与 TS 一样报格式错误，实际: {other:?}"),
    }
}

#[test]
fn delete_removes_only_target_key() {
    let dir = temp_dir("delete");
    let store = store(&dir);
    store.save("a", "1").unwrap();
    store.save("b", "2").unwrap();
    store.delete("a").unwrap();
    assert_eq!(store.load("a").unwrap(), None);
    assert_eq!(store.load("b").unwrap().as_deref(), Some("2"));
    // 删不存在的键是幂等的。
    store.delete("nope").unwrap();
}

#[test]
fn list_filters_prefix_and_sorts() {
    let dir = temp_dir("list");
    let store = store(&dir);
    for key in ["lan:b", "other", "lan:a", "lan:c"] {
        store.save(key, "v").unwrap();
    }
    assert_eq!(
        store.list("lan:").unwrap(),
        vec![
            "lan:a".to_string(),
            "lan:b".to_string(),
            "lan:c".to_string()
        ]
    );
    assert_eq!(store.list("").unwrap().len(), 4);
    assert!(store.list("missing:").unwrap().is_empty());
}

#[test]
fn list_does_not_decrypt_values() {
    let dir = temp_dir("list-no-decrypt");
    let store = store(&dir);
    store.save("good", "value").unwrap();
    // 手写一条结构合法的坏密文：读键名不应因为解密失败而失败。
    let mut entries: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&fs::read_to_string(store.config_file()).unwrap()).unwrap();
    entries.insert("bad".into(), "enc:v1:broken".into());
    fs::write(
        store.config_file(),
        serde_json::to_string_pretty(&entries).unwrap(),
    )
    .unwrap();

    assert_eq!(store.list("").unwrap(), vec!["bad", "good"]);
    assert!(matches!(
        store.load("bad"),
        Err(CredentialError::Decrypt { .. })
    ));
}

#[test]
fn plaintext_values_pass_through() {
    let dir = temp_dir("plaintext");
    let store = store(&dir);
    fs::write(store.config_file(), "{\"legacy\": \"raw-token\"}").unwrap();
    assert_eq!(store.load("legacy").unwrap().as_deref(), Some("raw-token"));
}

#[test]
fn corrupt_json_is_backed_up_and_never_overwritten() {
    let dir = temp_dir("corrupt");
    let store = store(&dir);
    let original = "{not json";
    fs::write(store.config_file(), original).unwrap();

    let error = store.load("k").unwrap_err();
    let CredentialError::Corrupt { backup, path, .. } = &error else {
        panic!("应当报损坏错误，实际: {error:?}");
    };
    assert_eq!(path, store.config_file());
    let backup = backup.as_ref().expect("必须留下备份");
    assert_eq!(fs::read_to_string(backup).unwrap(), original);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(backup).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "备份文件必须是 0600");
    }

    // 损坏文件禁止被 save 当空表覆盖。
    assert!(matches!(
        store.save("new", "value"),
        Err(CredentialError::Corrupt { .. })
    ));
    assert_eq!(fs::read_to_string(store.config_file()).unwrap(), original);

    // 同一份损坏内容重复读取只留一份备份。
    assert!(store.load("k").is_err());
    assert_eq!(count_backups(store.config_file()), 1);
}

#[test]
fn non_object_or_non_string_values_are_corrupt() {
    for (name, content) in [
        ("array-root", "[]"),
        ("null-root", "null"),
        ("number-root", "42"),
        ("non-string-value", "{\"k\": 1}"),
    ] {
        let dir = temp_dir(name);
        let store = store(&dir);
        fs::write(store.config_file(), content).unwrap();
        assert!(
            matches!(store.load("k"), Err(CredentialError::Corrupt { .. })),
            "{name} 应当按损坏处理"
        );
    }
}

#[test]
fn corrupt_listener_receives_paths_only() {
    use std::sync::{Arc, Mutex};

    type Seen = Vec<(PathBuf, Option<PathBuf>)>;
    let dir = temp_dir("listener");
    let seen: Arc<Mutex<Seen>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let store = CredentialStore::with_secret(&dir, SECRET).with_corrupt_listener(Arc::new(
        move |path: &Path, backup: Option<&Path>| {
            sink.lock().unwrap().push((
                path.to_path_buf(),
                backup.map(Path::to_path_buf),
            ));
        },
    ));
    fs::write(store.config_file(), "{broken").unwrap();

    assert!(store.load("k").is_err());
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, store.config_file());
    assert!(seen[0].1.is_some());
}

#[test]
fn decrypt_error_carries_ts_code() {
    let dir = temp_dir("code");
    let store = store(&dir);
    fs::write(store.config_file(), "{\"k\": \"enc:v1:broken\"}").unwrap();
    let error = store.load("k").unwrap_err();
    assert_eq!(error.code(), Some(CREDENTIAL_DECRYPT_ERROR_CODE));
    assert!(error.to_string().starts_with("凭据解密失败："));
}

#[test]
fn new_store_joins_v2_under_data_root() {
    let data_root = temp_dir("data-root");
    let store = CredentialStore::with_secret(&data_root, SECRET);
    assert_eq!(store.config_file(), data_root.join("credentials.json"));
    let from_root = CredentialStore::new(&data_root);
    assert_eq!(
        from_root.config_file(),
        data_root.join("v2").join("credentials.json")
    );
}

#[test]
fn concurrent_saves_do_not_lose_updates() {
    let dir = temp_dir("concurrent");
    let store = store(&dir);
    let mut handles = Vec::new();
    for worker in 0..4 {
        let store = store.clone();
        handles.push(std::thread::spawn(move || {
            for index in 0..10 {
                store
                    .save(&format!("w{worker}-{index}"), "value")
                    .expect("并发保存必须成功");
            }
        }));
    }
    for handle in handles {
        handle.join().unwrap();
    }

    let keys = store.list("").unwrap();
    assert_eq!(keys.len(), 40, "跨线程 read-modify-write 不得丢键");
    for worker in 0..4 {
        for index in 0..10 {
            assert!(keys.contains(&format!("w{worker}-{index}")));
        }
    }
    // 不留下临时文件。
    let temp_files = fs::read_dir(&dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
        .count();
    assert_eq!(temp_files, 0);
}
