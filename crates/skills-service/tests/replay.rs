//! fixture 回放对拍（`docs/specs/skills-service.md` §11）。
//!
//! `pnpm skills:fixtures` 用 TS 真源在临时沙盒里执行五个用例，记录六个方法的
//! 结果与服务结束后的整棵树；本测试在 Rust 侧重放同一批文件与操作序列，
//! 逐项深比较。字符串统一按生成脚本的归一化规则处理：沙盒根替换为
//! `<root>`、Windows 路径折叠为 `/`、技能 id 的 12 位哈希替换为 `<id-hash>`、
//! 审计日志键序排序且 `createdAt` 置为 `<ts>`。

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{json, Map, Value};

use skills_service::{
    ListParams, PromptContextParams, SetEnabledParams, SkillIdParams, SkillsService,
    SkillsServiceOptions,
};

const FIXTURE: &str = include_str!("../fixtures/skills.json");

#[derive(Deserialize)]
struct Fixture {
    cases: Vec<FixtureCase>,
}

#[derive(Deserialize)]
struct FixtureCase {
    name: String,
    desktop: bool,
    files: Vec<FixtureFile>,
    ops: Vec<FixtureOp>,
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
struct FixtureOp {
    kind: String,
    name: Option<String>,
    skill_id: Option<String>,
    enabled: Option<bool>,
    prompt: Option<String>,
}

/// 沙盒根的三种文本形式（native / 正斜杠 / 反斜杠翻倍），与生成脚本对齐。
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
            "zc-skills-replay-{tag}-{}-{nanos}",
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

    /// 生成脚本的 `normalizeText`：先替换翻倍反斜杠形态，再替换 native 与正斜杠形态；
    /// Windows 上最后把剩余反斜杠折叠为 `/`。
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

/// `realpathSync.native` 的对应物：canonicalize 后剥掉 Windows verbatim 前缀，
/// 避免临时目录的 8.3 短名与 realpath 展开后的长名不一致。
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

/// `/^(glm:[a-z]+:.+):[0-9a-f]{12}$/u` → `<前缀>:<id-hash>`。
fn normalize_skill_id(id: &str) -> String {
    let bytes = id.as_bytes();
    if bytes.len() < 14 {
        return id.to_string();
    }
    let tail_start = id.len() - 12;
    // 捕获组后必须紧跟 `:`，且末 12 位全为小写十六进制。
    if bytes[tail_start - 1] != b':' {
        return id.to_string();
    }
    if !bytes[tail_start..]
        .iter()
        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return id.to_string();
    }
    let prefix = &id[..tail_start - 1];
    let Some(rest) = prefix.strip_prefix("glm:") else {
        return id.to_string();
    };
    let Some((scope, remainder)) = rest.split_once(':') else {
        return id.to_string();
    };
    let scope_ok = !scope.is_empty() && scope.bytes().all(|byte| byte.is_ascii_lowercase());
    if !scope_ok || remainder.is_empty() {
        return id.to_string();
    }
    format!("{prefix}:<id-hash>")
}

/// 生成脚本的 `normalizeValue`：只有 `id` 键做哈希归一，其余字符串走根替换。
fn normalize_value(roots: &Roots, key: Option<&str>, value: &Value) -> Value {
    match value {
        Value::String(text) => {
            if key == Some("id") {
                Value::String(normalize_skill_id(text))
            } else {
                Value::String(roots.normalize_text(text))
            }
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| normalize_value(roots, None, item))
                .collect(),
        ),
        Value::Object(map) => {
            let mut out = Map::new();
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

/// `localeCompare` 的 ASCII 近似（与 crate `fsx::locale_compare` 同一口径），
/// 只用于树条目排序，保证与生成脚本的捕获顺序一致。
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

/// 生成脚本的 `normalizeAuditLog`：逐行解析、`createdAt` 置为 `<ts>`、
/// 键序排序（serde_json 默认 map 按键升序输出）、再走根替换。
fn normalize_audit_log(roots: &Roots, content: &str) -> String {
    content
        .split('\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            let record: Map<String, Value> =
                serde_json::from_str(line).expect("audit log line must be a JSON object");
            let mut record = record;
            record.insert("createdAt".to_string(), Value::String("<ts>".into()));
            roots.normalize_text(&Value::Object(record).to_string())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

struct Replay<'a> {
    roots: Roots,
    case: &'a FixtureCase,
    service: SkillsService,
}

impl<'a> Replay<'a> {
    fn materialize(&self) {
        for file in &self.case.files {
            let target = self.roots.native.join(&file.path);
            let parent = target.parent().expect("fixture path has a parent");
            std::fs::create_dir_all(parent).expect("materialize parent dirs");
            std::fs::write(&target, file.content.replace("<root>", &self.roots.forward))
                .expect("materialize file");
        }
    }

    fn list_params(&self) -> ListParams {
        ListParams {
            workspace_path: self.workspace(),
            workspace_identity: None,
        }
    }

    fn workspace(&self) -> PathBuf {
        self.roots.native.join("ws")
    }

    /// TS `resolveSkillId`：显式 `skillId` 优先，否则按 `list` 里的首个同名技能解析。
    fn resolve_skill_id(&self, op: &FixtureOp) -> Result<String, String> {
        if let Some(skill_id) = &op.skill_id {
            return Ok(skill_id.clone());
        }
        let name = op.name.clone().unwrap_or_default();
        let listed = self.service.list(&self.list_params());
        let found = listed
            .get("skills")
            .and_then(Value::as_array)
            .and_then(|skills| {
                skills
                    .iter()
                    .find(|skill| skill.get("name").and_then(Value::as_str) == Some(name.as_str()))
                    .cloned()
            });
        match found.and_then(|skill| skill.get("id").and_then(Value::as_str).map(String::from)) {
            Some(skill_id) => Ok(skill_id),
            None => Err(format!("skill not found by name: {name}")),
        }
    }

    fn run_op(&self, op: &FixtureOp) -> Result<Value, String> {
        match op.kind.as_str() {
            "list" => Ok(self.service.list(&self.list_params())),
            "buildPromptContext" => self
                .service
                .build_prompt_context(&PromptContextParams {
                    workspace_path: self.workspace(),
                    workspace_identity: None,
                    prompt: op.prompt.clone().unwrap_or_default(),
                })
                .map_err(|error| error.to_string()),
            "setEnabled" => {
                let skill_id = self.resolve_skill_id(op)?;
                self.service
                    .set_enabled(&SetEnabledParams {
                        workspace_path: self.workspace(),
                        workspace_identity: None,
                        skill_id,
                        enabled: op.enabled.unwrap_or(false),
                    })
                    .map(|()| json!({ "ok": true }))
                    .map_err(|error| error.to_string())
            }
            "copyToCommon" => {
                let skill_id = self.resolve_skill_id(op)?;
                self.service
                    .copy_to_common(&SkillIdParams {
                        workspace_path: self.workspace(),
                        workspace_identity: None,
                        skill_id,
                    })
                    .map_err(|error| error.to_string())
            }
            "removeFromCommon" => {
                let skill_id = self.resolve_skill_id(op)?;
                self.service
                    .remove_from_common(&SkillIdParams {
                        workspace_path: self.workspace(),
                        workspace_identity: None,
                        skill_id,
                    })
                    .map(|()| json!({ "ok": true }))
                    .map_err(|error| error.to_string())
            }
            "deleteSkill" => {
                let skill_id = self.resolve_skill_id(op)?;
                self.service
                    .delete_skill(&SkillIdParams {
                        workspace_path: self.workspace(),
                        workspace_identity: None,
                        skill_id,
                    })
                    .map(|()| json!({ "ok": true }))
                    .map_err(|error| error.to_string())
            }
            other => panic!("fixture 只包含六种操作，遇到 {other}"),
        }
    }

    /// TS `captureTree`：只收普通文件（软链不算），路径用 `/` 分隔，
    /// 按路径 `localeCompare` 升序，审计日志单独归一。
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
            let left_name = left.file_name().to_string_lossy().into_owned();
            let right_name = right.file_name().to_string_lossy().into_owned();
            locale_compare(&left_name, &right_name)
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
                let bytes = std::fs::read(entry.path()).expect("read tree file");
                let text = String::from_utf8_lossy(&bytes).into_owned();
                let content = if name.ends_with("skills-audit.log") {
                    normalize_audit_log(&self.roots, &text)
                } else {
                    self.roots.normalize_text(&text)
                };
                files.push((child_relative, content));
            }
        }
    }

    /// 返回该用例的全部差异（空 = 通过）。
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
                    "op[{index}] {:?} 结果不一致\n--- expected ---\n{}\n--- actual ---\n{}",
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
        let service = SkillsService::new(SkillsServiceOptions {
            home_dir: roots.native.join("home"),
            is_desktop_runtime: case.desktop,
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
