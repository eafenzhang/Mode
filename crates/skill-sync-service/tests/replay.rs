//! fixture 回放对拍（`docs/specs/skill-sync-service.md`）。
//!
//! `pnpm skill-sync:fixtures` 用 TS 真源生成候选、导出元数据与归档（base64 内嵌），
//! 以及基于该归档的导入结果；本测试重放并逐项深比较。gzip 字节不逐字节比较
//! （zlib 与 miniz_oxide 输出不同），导出只比较 skills 元数据。

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{json, Value};

use skill_sync_service::{SkillSyncService, SkillSyncServiceOptions};

const FIXTURE: &str = include_str!("../fixtures/skill-sync.json");

#[derive(Deserialize)]
struct Fixture {
    cases: Vec<FixtureCase>,
}

#[derive(Deserialize)]
struct FixtureCase {
    name: String,
    files: Vec<FixtureFile>,
    ops: Vec<Op>,
    expected: Vec<Value>,
    tree: Vec<FixtureFile>,
}

#[derive(Deserialize)]
struct FixtureFile {
    path: String,
    content: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Op {
    kind: String,
    directory_names: Option<Vec<String>>,
    skills: Option<Vec<SkillHint>>,
    names: Option<Vec<String>>,
    skill_ids: Option<Vec<String>>,
    archive_base64: Option<String>,
    overwrite: Option<bool>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SkillHint {
    directory_name: String,
    name: String,
}

struct Roots {
    native: PathBuf,
    native_text: String,
    forward: String,
    escaped: String,
}

impl Roots {
    fn create(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let base = std::env::temp_dir().join(format!(
            "zc-skill-sync-replay-{tag}-{}-{nanos}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("create sandbox");
        let native = canonical_long_path(&base);
        let native_text = native.to_string_lossy().into_owned();
        let forward = native_text.replace('\\', "/");
        let escaped = native_text.replace('\\', "\\\\");
        Self {
            native,
            native_text,
            forward,
            escaped,
        }
    }

    fn normalize_text(&self, text: &str) -> String {
        let mut replaced = text.replace(&self.escaped, "<root>");
        replaced = replaced.replace(&self.native_text, "<root>");
        replaced = replaced.replace(&self.forward, "<root>");
        if cfg!(windows) {
            replaced = replaced.replace("\\\\", "/");
            replaced = replaced.replace('\\', "/");
        }
        replaced
    }
}

fn canonical_long_path(path: &Path) -> PathBuf {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = canonical.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    canonical
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn normalize_value(roots: &Roots, key: Option<&str>, value: &Value) -> Value {
    match value {
        Value::String(text) => {
            if key == Some("id") && is_sha256(text) {
                Value::String("<sha256>".to_string())
            } else {
                Value::String(roots.normalize_text(text))
            }
        }
        Value::Number(_) if key == Some("archiveBytes") => Value::String("<bytes>".to_string()),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| normalize_value(roots, None, item))
                .collect(),
        ),
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (child_key, child) in map {
                out.insert(
                    child_key.clone(),
                    normalize_value(roots, Some(child_key), child),
                );
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

fn locale_compare(left: &str, right: &str) -> Ordering {
    let case_rank = |text: &str| -> Vec<u8> {
        text.chars()
            .map(|value| u8::from(value.is_uppercase()))
            .collect()
    };
    left.to_lowercase()
        .cmp(&right.to_lowercase())
        .then_with(|| case_rank(left).cmp(&case_rank(right)))
}

fn decode_base64(input: &str) -> Result<Vec<u8>, String> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut lookup = [255u8; 256];
    for (index, byte) in TABLE.iter().enumerate() {
        lookup[*byte as usize] = index as u8;
    }
    let mut output = Vec::new();
    let mut buffer: u32 = 0;
    let mut bits = 0u32;
    for byte in input.bytes() {
        if byte == b'=' || byte == b'\n' || byte == b'\r' {
            continue;
        }
        let value = lookup[byte as usize];
        if value == 255 {
            return Err("invalid base64".to_string());
        }
        buffer = (buffer << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push(((buffer >> bits) & 0xFF) as u8);
        }
    }
    Ok(output)
}

struct Replay<'a> {
    roots: Roots,
    case: &'a FixtureCase,
    service: SkillSyncService,
}

impl<'a> Replay<'a> {
    fn materialize(&self) {
        for file in &self.case.files {
            let target = self.roots.native.join(&file.path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).expect("materialize parent dirs");
            }
            std::fs::write(&target, file.content.clone()).expect("materialize file");
        }
    }

    fn resolve_ids(&self, names: &[String]) -> Result<Vec<String>, String> {
        let listed = self.service.list_local_user_skill_candidates();
        names
            .iter()
            .map(|name| {
                listed
                    .candidates
                    .iter()
                    .find(|candidate| &candidate.directory_name == name)
                    .map(|candidate| candidate.id.clone())
                    .ok_or_else(|| format!("skill sync candidate not found: {name}"))
            })
            .collect()
    }

    fn run_op(&self, op: &Op) -> Result<Value, String> {
        match op.kind.as_str() {
            "listCandidates" => Ok(serde_json::to_value(
                self.service.list_local_user_skill_candidates(),
            )
            .expect("candidates serialize")),
            "remoteStatuses" => {
                let hints: Vec<(String, String)> = op
                    .skills
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|hint| (hint.directory_name, hint.name))
                    .collect();
                let result = self.service.list_remote_user_skill_statuses(
                    &op.directory_names.clone().unwrap_or_default(),
                    &hints,
                )?;
                Ok(serde_json::to_value(&result).expect("statuses serialize"))
            }
            "export" => {
                let ids = match &op.names {
                    Some(names) => self.resolve_ids(names)?,
                    None => op.skill_ids.clone().unwrap_or_default(),
                };
                let result = self.service.export_skills_archive(&ids)?;
                Ok(json!({
                    "archive": "<archive>",
                    "archiveBytes": result.archive_bytes,
                    "skills": serde_json::to_value(&result.skills).expect("skills serialize"),
                }))
            }
            "import" => {
                let archive = decode_base64(op.archive_base64.as_deref().unwrap_or(""))?;
                let result = self
                    .service
                    .import_skills_archive(&archive, op.overwrite.unwrap_or(false))?;
                Ok(serde_json::to_value(&result).expect("import serializes"))
            }
            "writeAccess" => Ok(self.service.check_remote_user_skill_write_access()),
            other => panic!("fixture 只包含已知操作，遇到 {other}"),
        }
    }

    fn capture_tree(&self) -> Vec<(String, String)> {
        let mut files = Vec::new();
        self.walk_tree(&self.roots.native, "", &mut files);
        files.sort_by(|left, right| locale_compare(&left.0, &right.0));
        files
    }

    fn walk_tree(&self, directory: &Path, relative: &str, files: &mut Vec<(String, String)>) {
        let mut entries: Vec<std::fs::DirEntry> = std::fs::read_dir(directory)
            .expect("read tree directory")
            .flatten()
            .collect();
        entries.sort_by(|left, right| {
            locale_compare(
                &left.file_name().to_string_lossy(),
                &right.file_name().to_string_lossy(),
            )
        });
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            let child_relative = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
            let file_type = entry.file_type().expect("entry file type");
            if file_type.is_dir() {
                self.walk_tree(&entry.path(), &child_relative, files);
            } else if file_type.is_file() {
                let text =
                    String::from_utf8_lossy(&std::fs::read(entry.path()).expect("read tree file"))
                        .into_owned();
                files.push((child_relative, self.roots.normalize_text(&text)));
            }
        }
    }

    fn run(&self) -> Vec<String> {
        let mut failures = Vec::new();
        self.materialize();

        if self.case.expected.len() != self.case.ops.len() {
            failures.push(format!(
                "expected 数量 {} 与 ops 数量 {} 不一致",
                self.case.expected.len(),
                self.case.ops.len()
            ));
            return failures;
        }
        for (index, op) in self.case.ops.iter().enumerate() {
            let result = match self.run_op(op) {
                Ok(value) => value,
                Err(message) => json!({ "error": message }),
            };
            let actual = normalize_value(&self.roots, None, &result);
            let expected = &self.case.expected[index];
            if actual != *expected {
                failures.push(format!(
                    "op[{index}] {} 结果不一致\n--- expected ---\n{}\n--- actual ---\n{}",
                    op.kind,
                    serde_json::to_string_pretty(expected).unwrap_or_default(),
                    serde_json::to_string_pretty(&actual).unwrap_or_default(),
                ));
            }
        }

        let tree = self.capture_tree();
        if tree.len() != self.case.tree.len() {
            let expected_paths: Vec<&str> = self
                .case
                .tree
                .iter()
                .map(|file| file.path.as_str())
                .collect();
            let actual_paths: Vec<&str> = tree.iter().map(|(path, _)| path.as_str()).collect();
            failures.push(format!(
                "结束树文件数不一致：expected {:?}\nactual {:?}",
                expected_paths, actual_paths
            ));
            return failures;
        }
        for (index, (path, content)) in tree.iter().enumerate() {
            let expected = &self.case.tree[index];
            if path != &expected.path {
                failures.push(format!(
                    "结束树第 {index} 项路径不一致：expected {:?} actual {:?}",
                    expected.path, path
                ));
                continue;
            }
            if content != &expected.content {
                failures.push(format!(
                    "结束树 {path} 内容不一致\n--- expected ---\n{}\n--- actual ---\n{}",
                    expected.content, content
                ));
            }
        }
        failures
    }
}

#[test]
fn replay_ts_fixtures() {
    let fixture: Fixture = serde_json::from_str(FIXTURE).expect("fixture parses");
    let mut all_failures = Vec::new();
    for case in &fixture.cases {
        let roots = Roots::create(&case.name);
        let service = SkillSyncService::new(SkillSyncServiceOptions {
            home_dir: Some(roots.native.join("home")),
            max_archive_bytes: None,
        });
        let replay = Replay {
            roots,
            case,
            service,
        };
        for failure in replay.run() {
            all_failures.push(format!("[case {}] {failure}", case.name));
        }
        let _ = std::fs::remove_dir_all(&replay.roots.native);
    }
    assert!(
        all_failures.is_empty(),
        "fixture 回放存在差异：\n{}",
        all_failures.join("\n\n")
    );
}
