//! settings 文件层的行为用例：路径优先级、缺失/损坏、重试自愈、原子写回。

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use atomic_file::WriteOptions;
use settings_store::{
    read_document, resolve_settings_paths, write_document, HomeEnv, ReadOptions, ReadOutcome,
};

fn unique_home(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let home = std::env::temp_dir().join(format!(
        "mode-settings-store-{name}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&home);
    fs::create_dir_all(&home).expect("建临时 home");
    home
}

fn env_for(home: &Path) -> HomeEnv {
    HomeEnv {
        mode_desktop_home_dir: None,
        home: Some(OsString::from(home)),
        user_profile: None,
        fallback_home: home.to_path_buf(),
    }
}

#[test]
fn mode_desktop_home_dir_wins_over_home_and_user_profile() {
    let home = unique_home("precedence");
    let env = HomeEnv {
        mode_desktop_home_dir: Some(OsString::from(home.join("desktop"))),
        home: Some(OsString::from(home.join("home"))),
        user_profile: Some(OsString::from(home.join("profile"))),
        fallback_home: home.join("fallback"),
    };
    let paths = resolve_settings_paths(&env);
    assert_eq!(paths.home, home.join("desktop"));
    assert!(paths.file.ends_with(Path::new(".mode/v2/setting.json")));
}

#[test]
fn blank_values_fall_through_to_next_candidate() {
    let home = unique_home("blank");
    let env = HomeEnv {
        mode_desktop_home_dir: Some(OsString::from("   ")),
        home: Some(OsString::from("")),
        user_profile: None,
        fallback_home: home.clone(),
    };
    assert_eq!(resolve_settings_paths(&env).home, home);
}

#[test]
fn missing_file_reports_missing() {
    let home = unique_home("missing");
    let paths = resolve_settings_paths(&env_for(&home));
    assert!(matches!(
        read_document(&paths, &ReadOptions::default()),
        ReadOutcome::Missing
    ));
}

#[test]
fn loads_valid_json() {
    let home = unique_home("valid");
    let paths = resolve_settings_paths(&env_for(&home));
    fs::create_dir_all(&paths.dir).expect("建目录");
    fs::write(&paths.file, "{\"locale\":\"zh-CN\"}").expect("写文件");
    match read_document(&paths, &ReadOptions::default()) {
        ReadOutcome::Loaded(value) => assert_eq!(value["locale"], "zh-CN"),
        other => panic!("期望 Loaded，实际 {other:?}"),
    }
}

#[test]
fn quarantines_corrupt_file_after_retries() {
    let home = unique_home("corrupt");
    let paths = resolve_settings_paths(&env_for(&home));
    fs::create_dir_all(&paths.dir).expect("建目录");
    fs::write(&paths.file, ":wq").expect("写坏文件");

    let options = ReadOptions {
        parse_retry_count: 1,
        parse_retry_delay: Duration::from_millis(10),
    };
    match read_document(&paths, &options) {
        ReadOutcome::Corrupt { backup } => {
            let backup = backup.expect("应留下备份路径");
            assert_eq!(fs::read_to_string(&backup).expect("备份可读"), ":wq");
        }
        other => panic!("期望 Corrupt，实际 {other:?}"),
    }
    assert!(!paths.file.exists(), "坏文件必须被移走，不能每次启动都重复失败");
}

#[test]
fn retry_recovers_when_another_writer_finishes() {
    // 场景：读方撞上「另一个写者正在覆盖」的半截 JSON。重试期间文件变为合法内容，
    // 必须返回 Loaded 而不是把用户的配置隔离掉。
    let home = unique_home("retry");
    let paths = resolve_settings_paths(&env_for(&home));
    fs::create_dir_all(&paths.dir).expect("建目录");
    fs::write(&paths.file, "{\"half\":").expect("写半截");

    let writer_path = paths.file.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        fs::write(&writer_path, "{\"locale\":\"en-US\"}").expect("写完");
    });

    let options = ReadOptions {
        parse_retry_count: 2,
        parse_retry_delay: Duration::from_millis(200),
    };
    match read_document(&paths, &options) {
        ReadOutcome::Loaded(value) => assert_eq!(value["locale"], "en-US"),
        other => panic!("重试应自愈，实际 {other:?}"),
    }
}

#[test]
fn write_document_creates_dirs_and_pretty_json() {
    let home = unique_home("write");
    let paths = resolve_settings_paths(&env_for(&home));
    let value = serde_json::json!({ "b": 2, "a": 1 });
    write_document(&paths, &value, &WriteOptions::default()).expect("写入");
    let text = fs::read_to_string(&paths.file).expect("读回");
    assert!(!text.ends_with('\n'));
    assert!(text.contains("\n  \"a\": 1"), "应为两空格缩进：{text}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&text).expect("可解析"),
        value
    );
}

#[test]
fn ensure_default_project_is_idempotent() {
    let home = unique_home("project");
    let first = settings_store::ensure_default_project(&home).expect("首次创建");
    assert!(first.created, "首次应报告 created=true");
    assert_eq!(first.path, home.join("ModeProject"));
    assert!(first.path.is_dir());

    let second = settings_store::ensure_default_project(&home).expect("再次调用");
    assert!(!second.created, "已存在时应报告 created=false");
    assert_eq!(second.path, first.path);
}

#[test]
fn copy_data_directory_skips_setting_json_and_refuses_overwrite() {
    let old_base = unique_home("copy-old");
    let new_base = unique_home("copy-new");
    let v2 = old_base.join(".mode/v2");
    fs::create_dir_all(v2.join("nested")).expect("建源目录");
    fs::write(v2.join("keep.json"), "{}").expect("写文件");
    fs::write(v2.join("nested/deep.txt"), "x").expect("写嵌套文件");
    fs::write(v2.join("setting.json"), "{}").expect("写 setting");
    fs::write(v2.join("setting.json.lock"), "x").expect("写锁");
    fs::write(v2.join("setting.json.1.tmp"), "x").expect("写临时文件");

    settings_store::copy_data_directory(&old_base, &new_base).expect("复制成功");
    let copied = new_base.join(".mode/v2");
    assert!(copied.join("keep.json").is_file());
    assert!(copied.join("nested/deep.txt").is_file());
    assert!(!copied.join("setting.json").exists());
    assert!(!copied.join("setting.json.lock").exists());
    assert!(!copied.join("setting.json.1.tmp").exists());

    fs::write(copied.join("keep.json"), "conflict").expect("造冲突");
    assert!(
        settings_store::copy_data_directory(&old_base, &new_base).is_err(),
        "已存在的目标文件不能被覆盖"
    );
}

#[test]
fn windows_install_dir_validation_rejects_inside_paths() {
    let forbidden = PathBuf::from("C:/Program Files/Mode");
    assert!(
        settings_store::validate_data_base_dir_target(
            &PathBuf::from("C:/Program Files/Mode/data"),
            std::slice::from_ref(&forbidden),
            true
        )
        .is_err()
    );
    assert!(
        settings_store::validate_data_base_dir_target(
            &PathBuf::from("D:/ModeData"),
            std::slice::from_ref(&forbidden),
            true
        )
        .is_ok()
    );
    assert!(
        settings_store::validate_data_base_dir_target(
            &PathBuf::from("C:/Program Files/Mode/data"),
            std::slice::from_ref(&forbidden),
            false
        )
        .is_ok(),
        "非 Windows 不启用该限制"
    );
    assert!(settings_store::is_path_equal_or_inside(
        &PathBuf::from(r"C:\Program Files\Mode\data"),
        &forbidden
    ));
}

#[test]
fn update_data_base_dir_copies_and_persists_pointer() {
    let home = unique_home("update-db");
    fs::create_dir_all(home.join(".mode/v2")).expect("建旧数据目录");
    fs::write(home.join(".mode/v2/keep.json"), "{}").expect("写数据");

    let paths = resolve_settings_paths(&env_for(&home));
    let new_base = unique_home("update-db-new");
    let target = settings_store::update_data_base_dir(
        &paths,
        &home,
        Some(new_base.to_str().expect("utf8")),
        false,
        &[],
    )
    .expect("更新成功");

    assert_eq!(target, new_base);
    assert!(new_base.join(".mode/v2/keep.json").is_file(), "数据必须被复制");
    let document: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&paths.file).expect("读 setting")).expect("JSON");
    assert_eq!(
        document["dataBaseDir"],
        new_base.to_string_lossy().as_ref()
    );
}

#[test]
fn update_data_base_dir_rejects_forbidden_target_without_writing_settings() {
    let home = unique_home("update-forbidden");
    let paths = resolve_settings_paths(&env_for(&home));
    let forbidden = vec![std::path::PathBuf::from("C:/Program Files/Mode")];

    let error = settings_store::update_data_base_dir(
        &paths,
        &home,
        Some("C:/Program Files/Mode/data"),
        true,
        &forbidden,
    );
    assert!(error.is_err(), "安装目录内必须拒绝");
    assert!(!paths.file.exists(), "失败时不得写 setting.json");
}
