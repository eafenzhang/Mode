//! TS 真源对拍：用真实 git 重放 `scripts/generate-git-checkpoint-fixtures.ts` 的用例。
//!
//! 归一化口径与生成脚本一致：sandbox root → `<root>`；checkpoint id → `<checkpoint-N>`；
//! `createdAt` → `<ts>`；commit oid → `<hash>`；ref 内 12 位 workspace hash → `<workspaceHash>`。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use git_checkpoint_service::{GitCheckpointMeta, GitCheckpointService};
use git_cli::{GitCommandExecutionOptions, GitCommandProvider, GitEnvironmentProvider};
use serde_json::{json, Value};

static CASE_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn run_git(provider: &GitCommandProvider, cwd: &Path, args: &[&str]) {
    let options = GitCommandExecutionOptions::new(
        cwd.to_path_buf(),
        args.iter().map(|arg| arg.to_string()).collect(),
    );
    let result = provider.run(&options).expect("git command failed to spawn");
    assert_eq!(
        result.exit_code,
        Some(0),
        "git {:?} failed: {}",
        args,
        result.stderr
    );
}

fn materialize(root: &Path, files: &Value) {
    for file in files.as_array().cloned().unwrap_or_default() {
        let relative = file["path"].as_str().expect("file path");
        let target = root.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(target, file["content"].as_str().unwrap_or_default()).expect("write file");
    }
}

fn is_hash(text: &str) -> bool {
    text.len() == 40
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn replace_workspace_hashes(text: &str) -> String {
    const PREFIX: &str = "refs/mode/checkpoints/";
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find(PREFIX) {
        let (head, tail) = rest.split_at(index + PREFIX.len());
        output.push_str(head);
        let hash_len = tail
            .bytes()
            .take_while(|byte| byte.is_ascii_hexdigit())
            .count();
        if hash_len == 12 && tail.as_bytes().get(hash_len) == Some(&b'/') {
            output.push_str("<workspaceHash>/");
            rest = &tail[hash_len + 1..];
        } else {
            rest = tail;
        }
    }
    output.push_str(rest);
    output
}

fn normalize_text(root: &str, text: &str, ids: &[(String, String)]) -> String {
    let native = root;
    let forward = root.replace('\\', "/");
    let escaped = root.replace('\\', "\\\\");
    let mut replaced = text.replace(&escaped, "<root>");
    replaced = replaced.replace(native, "<root>");
    replaced = replaced.replace(&forward, "<root>");
    for (id, placeholder) in ids {
        replaced = replaced.replace(id, placeholder);
    }
    if cfg!(windows) {
        replaced = replaced.replace("\\\\", "/").replace('\\', "/");
    }
    replace_workspace_hashes(&replaced)
}

fn sort_string_array(value: &mut Value) {
    if let Value::Array(items) = value {
        items.sort_by_key(|item| item.as_str().unwrap_or_default().to_string());
    }
}

fn normalize_value(root: &str, ids: &[(String, String)], value: &Value) -> Value {    match value {
        Value::String(text) => {
            let replaced = normalize_text(root, text, ids);
            if is_hash(&replaced) {
                Value::String("<hash>".to_string())
            } else {
                Value::String(replaced)
            }
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| normalize_value(root, ids, item))
                .collect(),
        ),
        Value::Object(object) => {
            let mut output = serde_json::Map::new();
            for (key, child) in object {
                if key == "createdAt" {
                    output.insert(key.clone(), Value::String("<ts>".to_string()));
                    continue;
                }
                output.insert(key.clone(), normalize_value(root, ids, child));
            }
            Value::Object(output)
        }
        other => other.clone(),
    }
}

fn read_tree(root: &Path) -> Vec<(String, String)> {
    fn walk(dir: &Path, prefix: &str, output: &mut Vec<(String, String)>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .expect("read dir")
            .filter_map(Result::ok)
            .collect();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == ".git" {
                continue;
            }
            let absolute = entry.path();
            let relative = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if absolute.is_dir() {
                walk(&absolute, &relative, output);
            } else {
                let content = std::fs::read_to_string(&absolute).expect("read file");
                output.push((relative, content));
            }
        }
    }
    let mut output = Vec::new();
    walk(root, "", &mut output);
    output
}

fn unique_sandbox() -> PathBuf {
    let index = CASE_COUNTER.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!(
        "zc-git-checkpoint-replay-{}-{index}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("create sandbox");
    plugin_discovery::js::realpath(&path).expect("realpath sandbox")
}

fn checkpoint_id(metas: &[GitCheckpointMeta], index: Option<u64>) -> String {
    index
        .and_then(|index| metas.get(index as usize))
        .map(|meta| meta.checkpoint_id.clone())
        .unwrap_or_else(|| "missing-checkpoint".to_string())
}

fn run_case(case: &Value) {
    let name = case["name"].as_str().expect("case name");
    let sandbox = unique_sandbox();
    let root_text = sandbox.to_string_lossy().into_owned();
    std::fs::write(sandbox.join("gitconfig-empty"), "").expect("gitconfig");

    let provider = Arc::new(
        GitCommandProvider::new(Arc::new(GitEnvironmentProvider::new())).with_extra_env(vec![
            (
                "GIT_CONFIG_GLOBAL".to_string(),
                Some(
                    sandbox
                        .join("gitconfig-empty")
                        .to_string_lossy()
                        .into_owned(),
                ),
            ),
            (
                "GIT_CONFIG_SYSTEM".to_string(),
                Some(
                    sandbox
                        .join("gitconfig-empty")
                        .to_string_lossy()
                        .into_owned(),
                ),
            ),
            ("GIT_CONFIG_NOSYSTEM".to_string(), Some("1".to_string())),
            ("GIT_AUTHOR_NAME".to_string(), Some("Mode Test".to_string())),
            (
                "GIT_AUTHOR_EMAIL".to_string(),
                Some("mode@test.local".to_string()),
            ),
            (
                "GIT_COMMITTER_NAME".to_string(),
                Some("Mode Test".to_string()),
            ),
            (
                "GIT_COMMITTER_EMAIL".to_string(),
                Some("mode@test.local".to_string()),
            ),
            (
                "GIT_AUTHOR_DATE".to_string(),
                Some("2024-01-01T00:00:00+0000".to_string()),
            ),
            (
                "GIT_COMMITTER_DATE".to_string(),
                Some("2024-01-01T00:00:00+0000".to_string()),
            ),
        ]),
    );

    let workspace_segments: Vec<&str> = case["workspace"]
        .as_str()
        .expect("workspace")
        .split('/')
        .collect();
    let workspace = workspace_segments
        .iter()
        .fold(sandbox.clone(), |path, segment| path.join(segment));
    let repo_root = sandbox.join("ws");
    let setup = &case["setup"];
    let skip_repo = setup["skipRepo"].as_bool().unwrap_or(false);

    if skip_repo {
        std::fs::create_dir_all(&workspace).expect("workspace");
        materialize(&workspace, &setup["files"]);
    } else {
        std::fs::create_dir_all(&repo_root).expect("repo root");
        run_git(&provider, &repo_root, &["init", "-b", "main"]);
        run_git(&provider, &repo_root, &["config", "user.name", "Mode Test"]);
        run_git(
            &provider,
            &repo_root,
            &["config", "user.email", "mode@test.local"],
        );
        materialize(&repo_root, &setup["files"]);
        run_git(&provider, &repo_root, &["add", "-A"]);
        run_git(&provider, &repo_root, &["commit", "-m", "initial"]);
        if setup.get("extraFiles").is_some() {
            materialize(&repo_root, &setup["extraFiles"]);
        }
    }

    let service = GitCheckpointService::new(Arc::clone(&provider), &sandbox.join(".mode"));
    let mut checkpoints: Vec<GitCheckpointMeta> = Vec::new();
    let mut ids: Vec<(String, String)> = Vec::new();
    let expected = case["expected"].as_array().expect("expected");
    let ops = case["ops"].as_array().expect("ops");
    assert_eq!(expected.len(), ops.len(), "{name}: op/expected length");

    for (index, op) in ops.iter().enumerate() {
        let kind = op["kind"].as_str().expect("op kind");
        let actual: Result<Value, String> = match kind {
            "write" => (|| -> Result<Value, String> {
                let target = workspace.join(
                    op["path"]
                        .as_str()
                        .expect("write path")
                        .replace('/', std::path::MAIN_SEPARATOR_STR),
                );
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                std::fs::write(&target, op["content"].as_str().unwrap_or_default())
                    .map_err(|error| error.to_string())?;
                Ok(json!({ "ok": true }))
            })(),
            "remove" => {
                let target = workspace.join(
                    op["path"]
                        .as_str()
                        .expect("remove path")
                        .replace('/', std::path::MAIN_SEPARATOR_STR),
                );
                let _ = std::fs::remove_dir_all(&target);
                let _ = std::fs::remove_file(&target);
                Ok(json!({ "ok": true }))
            }
            "renameFile" => {
                let from = workspace.join(
                    op["from"]
                        .as_str()
                        .expect("rename from")
                        .replace('/', std::path::MAIN_SEPARATOR_STR),
                );
                let to = workspace.join(
                    op["to"]
                        .as_str()
                        .expect("rename to")
                        .replace('/', std::path::MAIN_SEPARATOR_STR),
                );
                std::fs::rename(&from, &to)
                    .map(|_| json!({ "ok": true }))
                    .map_err(|error| error.to_string())
            }
            "read" => {
                let target = workspace.join(
                    op["path"]
                        .as_str()
                        .expect("read path")
                        .replace('/', std::path::MAIN_SEPARATOR_STR),
                );
                std::fs::read_to_string(&target)
                    .map(|content| json!({ "content": content }))
                    .map_err(|error| error.to_string())
            }
            "create" => service.create_checkpoint(&workspace).map(|meta| {
                ids.push((
                    meta.checkpoint_id.clone(),
                    format!("<checkpoint-{}>", checkpoints.len()),
                ));
                checkpoints.push(meta.clone());
                serde_json::to_value(&meta).expect("meta json")
            }),
            "diff" => {
                let from = checkpoint_id(&checkpoints, op["from"].as_u64());
                let to = checkpoint_id(&checkpoints, op["to"].as_u64());
                service
                    .diff_checkpoints(&workspace, &from, &to)
                    .map(|diff| serde_json::to_value(&diff).expect("diff json"))
            }
            "restore" => {
                let from = checkpoint_id(&checkpoints, op["from"].as_u64());
                let to = checkpoint_id(&checkpoints, op["to"].as_u64());
                let force = op["force"].as_bool().unwrap_or(false);
                service
                    .restore_between_checkpoints(&workspace, &from, &to, force)
                    .map(|result| serde_json::to_value(&result).expect("restore json"))
            }
            "delete" => {
                let id = checkpoint_id(&checkpoints, op["checkpoint"].as_u64());
                service
                    .delete_checkpoint(&workspace, &id)
                    .map(|()| json!({ "ok": true }))
            }
            "refs" => {
                let options = GitCommandExecutionOptions::new(
                    repo_root.clone(),
                    vec![
                        "for-each-ref".to_string(),
                        "--format=%(refname)".to_string(),
                        "refs/mode/checkpoints".to_string(),
                    ],
                );
                provider.run(&options).map(|result| {
                    let refs: Vec<Value> = result
                        .stdout
                        .lines()
                        .filter(|line| !line.is_empty())
                        .map(|line| Value::String(line.to_string()))
                        .collect();
                    Value::Array(refs)
                })
            }
            other => panic!("{name}: unknown op {other}"),
        };

        let actual_value = match actual {
            Ok(value) => value,
            Err(error) => json!({ "error": error }),
        };
        let mut expected_value = normalize_value(&root_text, &ids, &expected[index]);
        let mut actual_value = normalize_value(&root_text, &ids, &actual_value);
        if kind == "refs" {
            // ref 名含随机 UUID，归一化后按占位符排序，避免生成/重放次序不稳定。
            sort_string_array(&mut expected_value);
            sort_string_array(&mut actual_value);
        }
        assert_eq!(
            actual_value, expected_value,
            "{name}: op #{index} ({kind}) mismatch"
        );
    }

    let expected_tree: Vec<(String, String)> = case["tree"]
        .as_array()
        .expect("tree")
        .iter()
        .map(|entry| {
            (
                entry["path"].as_str().expect("tree path").to_string(),
                entry["content"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert_eq!(
        read_tree(&workspace),
        expected_tree,
        "{name}: tree mismatch"
    );

    let _ = std::fs::remove_dir_all(&sandbox);
}

#[test]
fn replay_fixtures() {
    let fixture: Value =
        serde_json::from_str(include_str!("../fixtures/git-checkpoint-service.json"))
            .expect("fixture json");
    let cases = fixture["cases"].as_array().expect("cases");
    assert!(!cases.is_empty());
    for case in cases {
        run_case(case);
    }
}
