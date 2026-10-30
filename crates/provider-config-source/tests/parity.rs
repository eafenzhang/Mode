//! 与 TS 真源的对拍。
//!
//! fixture 由 `pnpm provider-config-source:fixtures` 生成：Personal/Builtin/Snapshot
//! 用例保存真实 TS 实现的 `JSON.stringify(snapshot)` 原文（绝对路径以 `<ROOT>`、
//! sourceKey 以 `<SOURCE_KEY>` 占位）。本测试在自建临时目录上重建同样布局，替换占位后
//! 逐字节比较；Personal 用例直接比较 revision 与字段顺序。

use std::path::{Path, PathBuf};

use provider_config_source::{
    read_mode_builtin_layer, read_personal_layer, read_snapshot, resolve_path_with_cwd,
    source_key_for_path, source_key_with_cwd,
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    version: u32,
    platform: String,
    root_placeholder: String,
    source_key_placeholder: String,
    resolve_cases: Vec<ResolveCase>,
    personal: Vec<PersonalCase>,
    builtin: Vec<BuiltinCase>,
    snapshot: Vec<SnapshotCase>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolveCase {
    name: String,
    cwd: String,
    input: String,
    expected_resolved: String,
    expected_source_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileRecord {
    path: String,
    content: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersonalCase {
    name: String,
    file_path: String,
    files: Vec<FileRecord>,
    expected_json: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuiltinCase {
    name: String,
    bundled: String,
    active: Option<String>,
    files: Vec<FileRecord>,
    expected_template: Option<String>,
    expect_error: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotCase {
    name: String,
    bundled: String,
    active: Option<String>,
    personal_path: String,
    files: Vec<FileRecord>,
    expected_template: String,
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/provider-config-source.json")
}

fn load_fixture() -> Fixture {
    let text = std::fs::read_to_string(fixture_path()).expect("读取 fixture");
    let fixture: Fixture = serde_json::from_str(&text).expect("解析 fixture");
    assert_eq!(fixture.version, 1, "fixture 版本");
    fixture
}

fn case_root(name: &str) -> PathBuf {
    let sanitized = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let root =
        std::env::temp_dir().join(format!("pcs-parity-{}-{}", std::process::id(), sanitized));
    std::fs::remove_dir_all(&root).ok();
    std::fs::create_dir_all(&root).expect("创建临时目录");
    root
}

fn write_files(root: &Path, files: &[FileRecord]) {
    for file in files {
        let target = root.join(&file.path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).expect("创建父目录");
        }
        std::fs::write(target, &file.content).expect("写入 fixture 文件");
    }
}

fn substitute(template: &str, placeholder: &str, value: &str) -> String {
    template.split(placeholder).collect::<Vec<_>>().join(value)
}

fn effective_active(bundled: &str, active: Option<&str>) -> String {
    active
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(bundled.trim())
        .to_owned()
}

#[test]
fn personal_layers_match_ts() {
    let fixture = load_fixture();
    for case in &fixture.personal {
        let root = case_root(&format!("personal-{}", case.name));
        write_files(&root, &case.files);
        let actual = read_personal_layer(root.join(&case.file_path));
        assert_eq!(
            actual.to_json_string(),
            case.expected_json,
            "personal case {}",
            case.name
        );
        std::fs::remove_dir_all(&root).ok();
    }
}

#[test]
fn builtin_layers_match_ts() {
    let fixture = load_fixture();
    for case in &fixture.builtin {
        let root = case_root(&format!("builtin-{}", case.name));
        write_files(&root, &case.files);
        let bundled = substitute(
            &case.bundled,
            &fixture.root_placeholder,
            &root.to_string_lossy(),
        );
        let active = case
            .active
            .as_ref()
            .map(|value| substitute(value, &fixture.root_placeholder, &root.to_string_lossy()));
        let result = read_mode_builtin_layer(&bundled, active.as_deref());
        if case.expect_error {
            assert!(result.is_err(), "builtin case {} 应失败", case.name);
        } else {
            let layer =
                result.unwrap_or_else(|error| panic!("builtin case {}: {error}", case.name));
            let expected_template = case
                .expected_template
                .as_ref()
                .unwrap_or_else(|| panic!("builtin case {} 缺少 expectedTemplate", case.name));
            let key = source_key_for_path(&effective_active(&bundled, active.as_deref()));
            let expected = substitute(expected_template, &fixture.source_key_placeholder, &key);
            assert_eq!(
                layer.to_json_string(),
                expected,
                "builtin case {}",
                case.name
            );
        }
        std::fs::remove_dir_all(&root).ok();
    }
}

#[test]
fn snapshots_match_ts() {
    let fixture = load_fixture();
    for case in &fixture.snapshot {
        let root = case_root(&format!("snapshot-{}", case.name));
        write_files(&root, &case.files);
        let bundled = substitute(
            &case.bundled,
            &fixture.root_placeholder,
            &root.to_string_lossy(),
        );
        let active = case
            .active
            .as_ref()
            .map(|value| substitute(value, &fixture.root_placeholder, &root.to_string_lossy()));
        let personal = root.join(&case.personal_path);
        let snapshot = read_snapshot(&bundled, &personal, active.as_deref())
            .unwrap_or_else(|error| panic!("snapshot case {}: {error}", case.name));
        let key = source_key_for_path(&effective_active(&bundled, active.as_deref()));
        let expected = substitute(
            &case.expected_template,
            &fixture.source_key_placeholder,
            &key,
        );
        assert_eq!(
            snapshot.to_json_string(),
            expected,
            "snapshot case {}",
            case.name
        );
        std::fs::remove_dir_all(&root).ok();
    }
}

#[test]
fn resolve_and_source_key_match_node() {
    let fixture = load_fixture();
    let host_platform = if cfg!(windows) { "win32" } else { "posix" };
    if fixture.platform != host_platform {
        eprintln!(
            "skip resolve cases: fixture platform {} != host {}",
            fixture.platform, host_platform
        );
        return;
    }
    for case in &fixture.resolve_cases {
        let resolved = resolve_path_with_cwd(&case.input, &case.cwd);
        assert_eq!(
            resolved, case.expected_resolved,
            "resolve case {}",
            case.name
        );
        assert_eq!(
            source_key_with_cwd(&case.input, &case.cwd),
            case.expected_source_key,
            "sourceKey case {}",
            case.name
        );
    }
}
