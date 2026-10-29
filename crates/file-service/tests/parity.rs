//! 与 TS 的黄金数据对拍：codec、搜索、过滤、`.modeignore` 与整树扫描。
//!
//! fixture 由 `pnpm file:fixtures` 用真实 TS 实现生成，Rust 侧只能设法通过。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use file_service::{
    filter_workspace_file_search_candidates, is_workspace_file_search_path_ignored,
    load_workspace_file_search_ignore_rules, map_entries_to_candidates, pack_entries,
    read_workspace_file_search_ignore, reset_workspace_file_search_ignore_defaults,
    sync_workspace_file_search_ignore_from_gitignore, transform_workspace_file_search_ignore,
    unpack_entries, workspace_file_search_decision, EntryType, FileService, FileServiceOptions,
    WorkspaceFileEntry, WORKSPACE_FILE_SEARCH_IGNORE_FILE_NAME,
};

const FIXTURE: &str = include_str!("../fixtures/file-fixtures.json");

fn fixture() -> serde_json::Value {
    serde_json::from_str(FIXTURE).expect("fixture 必须是合法 JSON")
}

fn entry_type(value: &serde_json::Value) -> EntryType {
    match value.as_str() {
        Some("directory") => EntryType::Directory,
        Some("file") => EntryType::File,
        other => panic!("非法 type: {other:?}"),
    }
}

fn entry(value: &serde_json::Value) -> WorkspaceFileEntry {
    WorkspaceFileEntry {
        name: value["name"].as_str().unwrap().to_string(),
        path: value["path"].as_str().unwrap().to_string(),
        relative_path: value["relativePath"].as_str().unwrap().to_string(),
        r#type: entry_type(&value["type"]),
    }
}

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-file-parity-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn codec_packs_and_unpacks_like_ts() {
    let fixture = fixture();
    let entries: Vec<WorkspaceFileEntry> = fixture["codec"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(entry)
        .collect();

    let packed = pack_entries(&entries);
    assert_eq!(packed, fixture["codec"]["packed"].as_str().unwrap());

    let posix: Vec<WorkspaceFileEntry> = fixture["codec"]["unpackPosix"]
        .as_array()
        .unwrap()
        .iter()
        .map(entry)
        .collect();
    assert_eq!(unpack_entries(&packed, "/ws"), posix);

    let windows: Vec<WorkspaceFileEntry> = fixture["codec"]["unpackWindows"]
        .as_array()
        .unwrap()
        .iter()
        .map(entry)
        .collect();
    assert_eq!(unpack_entries(&packed, "C:\\ws"), windows);
}

#[test]
fn search_results_match_ts() {
    let fixture = fixture();
    let entries: Vec<WorkspaceFileEntry> = fixture["search"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(entry)
        .collect();
    let candidates = map_entries_to_candidates(&entries);

    for case in fixture["search"]["queries"].as_array().unwrap() {
        let query = case["query"].as_str().unwrap();
        let limit = case["limit"].as_u64().unwrap() as usize;
        let require_query = case["requireQuery"].as_bool().unwrap();
        let actual: Vec<String> =
            filter_workspace_file_search_candidates(&candidates, query, Some(limit), require_query)
                .into_iter()
                .map(|candidate| candidate.relative_path)
                .collect();
        let expected: Vec<String> = case["expected"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_string())
            .collect();
        assert_eq!(actual, expected, "query={query:?} limit={limit}");
    }
}

#[test]
fn default_filter_decisions_match_ts() {
    let fixture = fixture();
    for case in fixture["filter"]["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let relative_path = case["relativePath"].as_str().unwrap();
        let entry_type = entry_type(&case["type"]);
        let active = case["ignoreRulesActive"].as_bool().unwrap();
        let actual = workspace_file_search_decision(name, relative_path, entry_type, active);
        assert_eq!(actual.include, case["decision"]["include"].as_bool().unwrap(), "{name}");
        assert_eq!(
            actual.traverse,
            case["decision"]["traverse"].as_bool().unwrap(),
            "{name}"
        );
    }
}

#[test]
fn ignore_matching_matches_ts() {
    let fixture = fixture();
    let ignore = &fixture["ignore"];
    let root = TempRoot::new("rules");
    fs::write(
        root.path().join(WORKSPACE_FILE_SEARCH_IGNORE_FILE_NAME),
        ignore["rulesContent"].as_str().unwrap(),
    )
    .unwrap();

    let outcome = futures_block_on(load_workspace_file_search_ignore_rules(root.path()));
    for case in ignore["matchCases"].as_array().unwrap() {
        let path = case["path"].as_str().unwrap();
        let entry_type = entry_type(&case["type"]);
        let actual = is_workspace_file_search_path_ignored(&outcome.rules, path, entry_type);
        assert_eq!(actual, case["ignored"].as_bool().unwrap(), "{path}");
    }
}

#[test]
fn read_and_load_sources_match_ts() {
    let fixture = fixture();
    let ignore = &fixture["ignore"];

    let rules_root = TempRoot::new("read-existing");
    fs::write(
        rules_root.path().join(WORKSPACE_FILE_SEARCH_IGNORE_FILE_NAME),
        ignore["rulesContent"].as_str().unwrap(),
    )
    .unwrap();
    let read = futures_block_on(read_workspace_file_search_ignore(rules_root.path())).unwrap();
    assert_eq!(read.content, ignore["readExisting"]["content"].as_str().unwrap());
    assert_eq!(read.source, ignore["readExisting"]["source"].as_str().unwrap());
    let loaded = futures_block_on(load_workspace_file_search_ignore_rules(rules_root.path()));
    assert_eq!(loaded.rules.source().as_str(), ignore["loadSourceExisting"].as_str().unwrap());

    let gitignore_root = TempRoot::new("read-gitignore");
    fs::write(
        gitignore_root.path().join(".gitignore"),
        ignore["createdFromGitignore"]["gitignoreContent"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let loaded = futures_block_on(load_workspace_file_search_ignore_rules(gitignore_root.path()));
    assert_eq!(
        loaded.rules.source().as_str(),
        ignore["createdFromGitignore"]["loadSource"]
            .as_str()
            .unwrap()
    );
    let read = futures_block_on(read_workspace_file_search_ignore(gitignore_root.path())).unwrap();
    assert_eq!(
        read.content,
        ignore["createdFromGitignore"]["read"]["content"]
            .as_str()
            .unwrap()
    );
    assert_eq!(
        read.source,
        ignore["createdFromGitignore"]["read"]["source"]
            .as_str()
            .unwrap()
    );

    let template_root = TempRoot::new("read-template");
    let loaded = futures_block_on(load_workspace_file_search_ignore_rules(template_root.path()));
    assert_eq!(
        loaded.rules.source().as_str(),
        ignore["createdFromTemplate"]["loadSource"]
            .as_str()
            .unwrap()
    );
    let read = futures_block_on(read_workspace_file_search_ignore(template_root.path())).unwrap();
    assert_eq!(
        read.content,
        ignore["createdFromTemplate"]["read"]["content"]
            .as_str()
            .unwrap()
    );
    assert_eq!(
        read.source,
        ignore["createdFromTemplate"]["read"]["source"]
            .as_str()
            .unwrap()
    );
}

#[test]
fn sync_and_reset_match_ts() {
    let fixture = fixture();
    let ignore = &fixture["ignore"];
    for (kind, cases) in [("sync", &ignore["syncCases"]), ("reset", &ignore["resetCases"])] {
        for case in cases.as_array().unwrap() {
            let current = case["current"].as_str().unwrap();
            let gitignore = case["gitignore"].as_str().unwrap();
            let expected = case["expected"].as_str().unwrap();
            let actual = if kind == "sync" {
                sync_workspace_file_search_ignore_from_gitignore(current, Some(gitignore))
            } else {
                reset_workspace_file_search_ignore_defaults(current, Some(gitignore))
            };
            assert_eq!(actual, expected, "{kind} case");
        }
    }
}

#[test]
fn transform_without_markers_matches_ts() {
    let fixture = fixture();
    let root = TempRoot::new("transform");
    fs::write(root.path().join(".gitignore"), "dist/\n").unwrap();
    fs::write(
        root.path().join(WORKSPACE_FILE_SEARCH_IGNORE_FILE_NAME),
        "custom-rule\n",
    )
    .unwrap();
    let content =
        futures_block_on(transform_workspace_file_search_ignore(root.path(), "sync-gitignore"))
            .unwrap();
    assert_eq!(
        content,
        fixture["ignore"]["transformNoMarkers"]["content"]
            .as_str()
            .unwrap()
    );
}

#[test]
fn tree_scan_matches_ts() {
    let fixture = fixture();
    let tree = &fixture["tree"];
    let root = TempRoot::new("tree");
    for dir in tree["dirs"].as_array().unwrap() {
        fs::create_dir_all(root.path().join(dir.as_str().unwrap())).unwrap();
    }
    for file in tree["files"].as_array().unwrap() {
        fs::write(root.path().join(file.as_str().unwrap()), "").unwrap();
    }
    fs::write(
        root.path().join(WORKSPACE_FILE_SEARCH_IGNORE_FILE_NAME),
        tree["modeignore"].as_str().unwrap(),
    )
    .unwrap();

    let service = FileService::new(FileServiceOptions {
        data_root: root.path().to_path_buf(),
        home: root.path().to_path_buf(),
        on_warn: None,
    });
    let root_path = root.path().to_string_lossy().into_owned();
    let length = futures_block_on(async {
        service
            .list_workspace_files_length(&file_service::RootParams {
                root_path: root_path.clone(),
            })
            .await
            .unwrap()
    });
    assert_eq!(length, tree["length"].as_u64().unwrap() as usize);

    let packed = futures_block_on(async {
        service
            .list_workspace_files_range(&file_service::RootRangeParams {
                root_path,
                offset: 0.0,
                length: 10_000_000.0,
            })
            .await
            .unwrap()
    });
    assert_eq!(packed, tree["packed"].as_str().unwrap());
}

/// 测试内的小型执行器（避免为每条用例引入 `#[tokio::test]` + 运行时样板）。
fn futures_block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}
