//! 与 TS `data-root` 归属逻辑的对拍：`pnpm data-root:fixtures` 生成夹具。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use data_root::{
    cleanup_stale_migration_staging_with, collect_legacy_candidate_stats,
    discover_legacy_data_root_candidates, execute_data_root_copy_migration,
    forfeit_conflicting_data_root, forfeit_data_root_by_label, is_accepted_data_root_product,
    parse_data_root_manifest, read_data_root_status, read_legacy_data_base_dir_from_settings,
    write_data_root_manifest_into_root, CorruptReason, DataRootManifest, DataRootManifestCreatedBy,
    DataRootManifestMigrationMode, DataRootMigrationProgress, DataRootMigrationResult,
    DataRootStatus, ExecuteDataRootCopyMigrationInput, UnownedReason,
};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Mutex;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    manifest_parse: Vec<ManifestParseCase>,
    product: Vec<ProductCase>,
    status: Vec<StatusCase>,
    write: Vec<WriteCase>,
    forfeit: Vec<ForfeitCase>,
    legacy_settings: Vec<LegacyCase>,
    discovery: Vec<DiscoveryCase>,
    stats: Vec<StatsCase>,
    cleanup: Vec<CleanupCase>,
    copy_migration: Vec<CopyMigrationCase>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CopyMigrationCase {
    name: String,
    files: BTreeMap<String, String>,
    dirs: Vec<String>,
    mode: String,
    cancel: bool,
    compare_progress: bool,
    expected: CopyMigrationExpected,
}

#[derive(Debug, Deserialize)]
struct CopyMigrationExpected {
    result: Value,
    tree: Vec<Value>,
    progress: Option<Vec<Value>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiscoveryCase {
    name: String,
    files: BTreeMap<String, String>,
    dirs: Vec<String>,
    expected: Vec<CandidateExpected>,
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CandidateExpected {
    base_dir: String,
    legacy_root: String,
    is_primary_base: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatsCase {
    name: String,
    files: BTreeMap<String, String>,
    expected: StatsExpected,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatsExpected {
    size_bytes: u64,
    item_count: u64,
    modified_at_present: bool,
}

#[derive(Debug, Deserialize)]
struct CleanupCase {
    name: String,
    entries: Vec<CleanupEntry>,
    expected: CleanupExpected,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CleanupEntry {
    name: String,
    age_ms: i64,
    directory: bool,
}

#[derive(Debug, Deserialize)]
struct CleanupExpected {
    removed: Vec<String>,
    surviving: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ManifestParseCase {
    name: String,
    raw: String,
    expected: Option<DataRootManifest>,
}

#[derive(Debug, Deserialize)]
struct ProductCase {
    product: String,
    expected: bool,
}

#[derive(Debug, Deserialize)]
struct StatusCase {
    name: String,
    files: BTreeMap<String, String>,
    dirs: Vec<String>,
    expected: Value,
}

#[derive(Debug, Deserialize)]
struct WriteCase {
    manifest: DataRootManifest,
    expected: WriteExpected,
}

#[derive(Debug, Deserialize)]
struct WriteExpected {
    path: String,
    content: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ForfeitCase {
    name: String,
    call: String,
    label: String,
    setup: bool,
    expected: ForfeitExpected,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ForfeitExpected {
    backup: Option<String>,
    root_exists_after: bool,
}

#[derive(Debug, Deserialize)]
struct LegacyCase {
    name: String,
    content: Option<String>,
    expected: Option<String>,
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-data-root-fixture-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp base");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn materialize(base: &Path, files: &BTreeMap<String, String>, dirs: &[String]) {
    for dir in dirs {
        std::fs::create_dir_all(base.join(dir)).expect("create dir");
    }
    for (relative, content) in files {
        let target = base.join(relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        let content = content
            .replace("<root>", &base.to_string_lossy())
            .replace("<rootForward>", &base.to_string_lossy().replace('\\', "/"));
        std::fs::write(&target, content).expect("write file");
    }
}

fn normalize_value(value: &mut Value, root: &str) {
    match value {
        Value::String(text) => *text = text.replace(root, "<root>"),
        Value::Array(items) => {
            for item in items {
                normalize_value(item, root);
            }
        }
        Value::Object(map) => {
            for child in map.values_mut() {
                normalize_value(child, root);
            }
        }
        _ => {}
    }
}

fn normalize_backup(text: &str, root: &str) -> String {
    text.replace(root, "<root>")
        .replace("20260101-000000", "<ts>")
}

#[test]
fn matches_ts_data_root_ownership() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("data-root.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 {} 失败: {error}", path.display()));
    let fixture: Fixture =
        serde_json::from_str(&raw).unwrap_or_else(|error| panic!("解析夹具失败: {error}"));

    for case in &fixture.manifest_parse {
        assert_eq!(
            parse_data_root_manifest(&case.raw),
            case.expected,
            "{}: manifest parse",
            case.name
        );
    }
    for case in &fixture.product {
        assert_eq!(
            is_accepted_data_root_product(&case.product),
            case.expected,
            "{}: product",
            case.product
        );
    }
    for case in &fixture.status {
        let temp = TempDir::new();
        materialize(&temp.0, &case.files, &case.dirs);
        let status = read_data_root_status(&temp.0);
        let mut actual = serde_json::to_value(status).expect("序列化 status");
        let root_text = temp.0.to_string_lossy().into_owned();
        normalize_value(&mut actual, &root_text);
        assert_eq!(actual, case.expected, "{}: status", case.name);
    }
    for case in &fixture.write {
        let temp = TempDir::new();
        let root_text = temp.0.to_string_lossy().into_owned();
        let root_dir = temp.0.join(".mode");
        let path = write_data_root_manifest_into_root(&root_dir, &case.manifest)
            .unwrap_or_else(|error| panic!("写入失败: {error}"));
        assert_eq!(
            path.replace(&root_text, "<root>"),
            case.expected.path,
            "write path"
        );
        let content = std::fs::read_to_string(root_dir.join(".mode-root.json")).expect("read");
        assert_eq!(content, case.expected.content, "write content");
    }
    for case in &fixture.forfeit {
        let temp = TempDir::new();
        let root_text = temp.0.to_string_lossy().into_owned();
        if case.setup {
            std::fs::create_dir_all(temp.0.join(".mode")).expect("create .mode");
        }
        let backup = if case.call == "label" {
            forfeit_data_root_by_label(&temp.0, &case.label, "20260101-000000")
        } else {
            let status = if case.label == "corrupt" {
                DataRootStatus::Corrupt {
                    reason: CorruptReason::ManifestUnreadable,
                    root_dir: temp.0.join(".mode").to_string_lossy().into_owned(),
                }
            } else {
                DataRootStatus::Unowned {
                    reason: UnownedReason::ManifestMissing,
                    root_dir: temp.0.join(".mode").to_string_lossy().into_owned(),
                }
            };
            forfeit_conflicting_data_root(&temp.0, &status, "20260101-000000")
        }
        .unwrap_or_else(|error| panic!("{}: forfeit 失败: {error}", case.name));
        let backup = backup.map(|path| normalize_backup(&path, &root_text));
        assert_eq!(backup, case.expected.backup, "{}: backup", case.name);
        assert_eq!(
            temp.0.join(".mode").exists(),
            case.expected.root_exists_after,
            "{}: root exists after",
            case.name
        );
    }
    for case in &fixture.legacy_settings {
        let temp = TempDir::new();
        let legacy_root = temp.0.join("legacy");
        std::fs::create_dir_all(legacy_root.join("v2")).expect("create v2");
        if let Some(content) = &case.content {
            std::fs::write(legacy_root.join("v2/setting.json"), content).expect("write settings");
        }
        assert_eq!(
            read_legacy_data_base_dir_from_settings(&legacy_root),
            case.expected,
            "{}: legacy dataBaseDir",
            case.name
        );
    }
}

#[test]
fn matches_ts_data_root_migration_primitives() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("data-root.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 {} 失败: {error}", path.display()));
    let fixture: Fixture =
        serde_json::from_str(&raw).unwrap_or_else(|error| panic!("解析夹具失败: {error}"));

    for case in &fixture.discovery {
        let temp = TempDir::new();
        let root = temp.0.clone();
        materialize(&root, &case.files, &case.dirs);
        let home = root.join("home");
        std::fs::create_dir_all(&home).expect("create home");
        let root_text = root.to_string_lossy().into_owned();
        let candidates = discover_legacy_data_root_candidates(&root, &home);
        let actual: Vec<CandidateExpected> = candidates
            .iter()
            .map(|candidate| CandidateExpected {
                base_dir: candidate
                    .base_dir
                    .to_string_lossy()
                    .replace(&root_text, "<root>"),
                legacy_root: candidate
                    .legacy_root
                    .to_string_lossy()
                    .replace(&root_text, "<root>"),
                is_primary_base: candidate.is_primary_base,
            })
            .collect();
        assert_eq!(actual, case.expected, "{}: discovery", case.name);
    }

    for case in &fixture.stats {
        let temp = TempDir::new();
        materialize(&temp.0, &case.files, &[]);
        let stats = collect_legacy_candidate_stats(&temp.0);
        assert_eq!(
            stats.size_bytes, case.expected.size_bytes,
            "{}: size",
            case.name
        );
        assert_eq!(
            stats.item_count, case.expected.item_count,
            "{}: items",
            case.name
        );
        assert_eq!(
            stats.modified_at.is_some(),
            case.expected.modified_at_present,
            "{}: modifiedAt",
            case.name
        );
    }

    let now = 1_700_000_000_000i64;
    for case in &fixture.cleanup {
        let temp = TempDir::new();
        let root = temp.0.clone();
        for entry in &case.entries {
            let target = root.join(&entry.name);
            if entry.directory {
                std::fs::create_dir_all(&target).expect("create dir");
            } else {
                std::fs::write(&target, b"").expect("create file");
            }
        }
        let ages: BTreeMap<String, i64> = case
            .entries
            .iter()
            .map(|entry| (entry.name.clone(), entry.age_ms))
            .collect();
        let mtime = |path: &Path| {
            let name = path.file_name()?.to_string_lossy().into_owned();
            ages.get(&name).map(|age| now - age)
        };
        let removed = cleanup_stale_migration_staging_with(&root, now, 3_600_000, &mtime);
        let root_text = root.to_string_lossy().into_owned();
        let normalized_removed: Vec<String> = removed
            .iter()
            .map(|path| path.replace(&root_text, "<root>"))
            .collect();
        assert_eq!(
            normalized_removed, case.expected.removed,
            "{}: removed",
            case.name
        );
        let mut surviving: Vec<String> = std::fs::read_dir(&root)
            .expect("read dir")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        surviving.sort();
        assert_eq!(
            surviving, case.expected.surviving,
            "{}: surviving",
            case.name
        );
    }
}

fn normalize_copy_text(text: &str, root: &str) -> String {
    text.replace(root, "<root>")
        .replace(&root.replace('\\', "\\\\"), "<root>")
        .replace(&root.replace('\\', "/"), "<root>")
        .replace("2026-01-01T00:00:00.000Z", "<ts>")
        .replace("20260101-000000", "<ts>")
}

fn normalize_copy_value(value: &mut Value, root: &str) {
    match value {
        Value::String(text) => *text = normalize_copy_text(text, root),
        Value::Array(items) => {
            for item in items {
                normalize_copy_value(item, root);
            }
        }
        Value::Object(map) => {
            for child in map.values_mut() {
                normalize_copy_value(child, root);
            }
        }
        _ => {}
    }
}

fn copy_tree_snapshot(root: &Path, root_text: &str) -> Vec<Value> {
    fn walk(dir: &Path, relative: &str, root_text: &str, entries: &mut Vec<Value>) {
        let mut children: Vec<_> = std::fs::read_dir(dir)
            .expect("read dir")
            .flatten()
            .collect();
        children.sort_by_key(|entry| entry.file_name());
        for entry in children {
            let name = entry.file_name().to_string_lossy().into_owned();
            let child_relative = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
            let target = dir.join(&name);
            let path = normalize_copy_text(&child_relative, root_text);
            if target.is_dir() {
                entries.push(serde_json::json!({ "path": path, "kind": "dir" }));
                walk(&target, &child_relative, root_text, entries);
            } else if target.is_file() {
                let content = std::fs::read_to_string(&target).unwrap_or_default();
                entries.push(serde_json::json!({
                    "path": path,
                    "kind": "file",
                    "content": normalize_copy_text(&content, root_text),
                }));
            }
        }
    }
    let mut entries = Vec::new();
    walk(root, "", root_text, &mut entries);
    entries.sort_by(|left, right| {
        left["path"]
            .as_str()
            .unwrap_or_default()
            .cmp(right["path"].as_str().unwrap_or_default())
    });
    entries
}

#[test]
fn matches_ts_data_root_copy_migration() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("data-root.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 {} 失败: {error}", path.display()));
    let fixture: Fixture =
        serde_json::from_str(&raw).unwrap_or_else(|error| panic!("解析夹具失败: {error}"));

    for case in &fixture.copy_migration {
        assert_eq!(
            case.compare_progress,
            case.expected.progress.is_some(),
            "{}: compareProgress 与夹具不一致",
            case.name
        );
        let temp = TempDir::new();
        let root = temp.0.clone();
        materialize(&root, &case.files, &case.dirs);
        let home = root.join("home");
        std::fs::create_dir_all(&home).expect("create home");
        let root_text = root.to_string_lossy().into_owned();
        let candidates = discover_legacy_data_root_candidates(&root, &home);
        let progress = Mutex::new(Vec::new());
        let mode = match case.mode.as_str() {
            "import" => DataRootManifestMigrationMode::Import,
            _ => DataRootManifestMigrationMode::Copy,
        };
        let cancel = case.cancel;
        let staging_id = || "staging-test".to_string();
        let now_iso = || "2026-01-01T00:00:00.000Z".to_string();
        let backup_timestamp = || "20260101-000000".to_string();
        let on_progress = |event: &DataRootMigrationProgress| {
            progress
                .lock()
                .expect("progress lock")
                .push(serde_json::to_value(event).expect("序列化 progress"));
        };
        let is_cancelled = || cancel;
        let result = execute_data_root_copy_migration(&ExecuteDataRootCopyMigrationInput {
            candidates: &candidates,
            created_by: DataRootManifestCreatedBy::Desktop,
            app_version: "0.0.1",
            mode,
            on_progress: &on_progress,
            is_cancelled: &is_cancelled,
            staging_id: &staging_id,
            now_iso: &now_iso,
            backup_timestamp: &backup_timestamp,
        });
        let mut actual_result = match &result {
            DataRootMigrationResult::Ok {
                migrated_bases,
                manifest_paths,
            } => serde_json::json!({
                "ok": true,
                "migratedBases": migrated_bases,
                "manifestPaths": manifest_paths,
            }),
            DataRootMigrationResult::Err { error, cancelled } => {
                serde_json::json!({ "ok": false, "error": error, "cancelled": cancelled })
            }
        };
        normalize_copy_value(&mut actual_result, &root_text);
        assert_eq!(actual_result, case.expected.result, "{}: result", case.name);

        let tree = copy_tree_snapshot(&root, &root_text);
        assert_eq!(tree, case.expected.tree, "{}: tree", case.name);

        if let Some(expected_progress) = &case.expected.progress {
            let mut actual_progress = progress.lock().expect("progress lock").clone();
            for event in &mut actual_progress {
                normalize_copy_value(event, &root_text);
            }
            assert_eq!(
                actual_progress, *expected_progress,
                "{}: progress",
                case.name
            );
        }
    }
}
