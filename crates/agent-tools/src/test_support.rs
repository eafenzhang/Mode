//! 测试支撑：把 Rust 侧结果**投影成夹具里的同一形状**，供 `agent-tools` 与 `agent-core` 的对拍
//! 用例共用。
//!
//! 为什么它住在库里而不是各自的 `tests/`：spec §3 的规则表只有**一份**实现（TS 侧
//! `normalize.mts`，供 TS 发起的对拍管线用），Rust 侧按 Ruling 4 只做「投影后精确相等」。
//! 如果两个 crate 各写一份投影，那两份必然会漂——正是 Ruling 4 要避免的漂移，只是换了个位置。
//! 所以投影只写一次，放在这里，两个 crate 的用例都调它。
//!
//! 本模块**不**新增白名单规则、也**不**做部分匹配：规则逐条对应 spec §3 的表
//! （进程内路径前缀、平铺的 uuid），比较方式是 `assert_eq!`。
#![doc(hidden)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;
use sha2::{Digest, Sha256};

/// 一条脚本化的工具调用（夹具 `script[*].toolCalls[*]`）。
#[derive(Debug, Clone)]
pub struct FixtureCall {
    pub name: String,
    pub input: Value,
}

/// 夹具里录制的一条工具调用结果（`expect.toolCalls[*]`，已是规范化形状）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedCall {
    pub name: String,
    pub ok: bool,
    pub output: String,
}

/// 一份行为夹具里本套件用得到的部分。
#[derive(Debug)]
pub struct Fixture {
    pub case: String,
    /// 种子文件（文本 + 原始字节），`(相对路径, 内容)`。
    pub seed: Vec<(String, Vec<u8>)>,
    /// 剧本里的工具调用序列。
    pub calls: Vec<FixtureCall>,
    /// 录制的期望值。
    pub expect_calls: Vec<ExpectedCall>,
    /// 录制的**规范化入参**（`expect.toolCalls[*].input`），用来验投影规则本身。
    pub expect_inputs: Vec<Value>,
    /// 第二层（文件快照）的键集。
    pub expect_file_keys: Vec<String>,
    /// 第二层（文件快照）的完整投影：相对路径 → 内容 **sha256**（与 TS 侧
    /// `normalize.mts` 的 `takeFileSnapshot` 同一口径，见 [`snapshot_directory`]）。
    ///
    /// 比 `expect_file_keys` 强：写工具会**新增**文件，只比键集会漏掉内容错。
    pub expect_files: BTreeMap<String, String>,
    /// 每个 turn 里的工具调用条数（首个 turn 在前）。单 turn 夹具就是 `[全部]`。
    ///
    /// 「跨 turn」的观察（`Read` 的重复读去重）只能靠多 turn 夹具钉住，见 `fixtures.mts` 的注释。
    pub turn_sizes: Vec<usize>,
    /// 每个调用的**结果对象**里那四个对拍要读的字段（`success`/`truncated`/两个字节计数/预算策略），
    /// 从 `expect.events` 的 `tool.updated`/`result` 载荷里取（`None` = 该调用以错误结束）。
    pub expect_result_objects: Vec<Option<ResultObject>>,
}

/// 录制到的 `tool.updated`/`result` 对象（只取对拍要用的字段）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultObject {
    pub truncated: bool,
    pub original_bytes: u64,
    pub returned_bytes: u64,
    pub budget_strategy: String,
}

/// 行为夹具目录（`packages/desktop/tests/agent-runtime-oracle/fixtures`）。
pub fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("packages/desktop/tests/agent-runtime-oracle/fixtures")
}

/// 第二层（文件快照）的 Rust 投影：递归走 `root`，键是**正斜杠分隔的相对路径**，值是内容
/// **sha256 十六进制**。
///
/// 与 TS 侧 `normalize.mts` 的 `takeFileSnapshot` 逐条对齐：同一组跳过目录
/// （`.git` / `node_modules` / `.mode`）、同样只收普通文件、同样的键形状。规则**只此一份**
/// （Ruling 4：两份投影必然漂），TS 侧那一份是同一条规则的另一种语言。
pub fn snapshot_directory(root: &Path) -> BTreeMap<String, String> {
    const SKIP_DIRS: [&str; 3] = [".git", "node_modules", ".mode"];
    let mut snapshot = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(current) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let absolute = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if absolute.is_dir() {
                if SKIP_DIRS.contains(&name.as_str()) {
                    continue;
                }
                pending.push(absolute);
                continue;
            }
            if !absolute.is_file() {
                continue;
            }
            let Ok(content) = std::fs::read(&absolute) else {
                continue;
            };
            let relative = absolute
                .strip_prefix(root)
                .expect("走过的路径都在 root 下")
                .to_string_lossy()
                .replace('\\', "/");
            let digest = Sha256::digest(&content);
            snapshot.insert(relative, format!("{digest:x}"));
        }
    }
    snapshot
}

/// 读一份行为夹具。缺文件直接 panic（夹具不在仓库里是配置错误，不是「跳过」）。
pub fn load_fixture(case: &str) -> Fixture {
    let path = fixture_dir().join(format!("{case}.json"));
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读不到夹具 {}：{error}", path.display()));
    let json: Value = serde_json::from_str(&raw).expect("夹具是合法 JSON");

    let mut seed: Vec<(String, Vec<u8>)> = json["seed"]
        .as_object()
        .map(|object| {
            object
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().expect("种子是文本").as_bytes().to_vec(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    if let Some(object) = json["seedBytesHex"].as_object() {
        for (key, value) in object {
            seed.push((
                key.clone(),
                hex_to_bytes(value.as_str().expect("十六进制文本")),
            ));
        }
    }

    if let Some(object) = json["generatedSeed"].as_object() {
        for (key, spec) in object {
            let width = spec["width"].as_u64().expect("width") as usize;
            let count = spec["lines"].as_u64().expect("lines") as usize;
            let character = spec["char"].as_str().expect("char");
            let line = character.repeat(width);
            let content = std::iter::repeat_n(line, count).collect::<Vec<_>>().join(
                "
",
            );
            seed.push((key.clone(), content.into_bytes()));
        }
    }

    let mut calls = Vec::new();
    for step in json["script"].as_array().expect("script 是数组") {
        let Some(list) = step.get("toolCalls").and_then(Value::as_array) else {
            continue;
        };
        for call in list {
            calls.push(FixtureCall {
                name: call["name"].as_str().expect("工具名是字符串").to_string(),
                input: call["input"].clone(),
            });
        }
    }

    let expect_calls: Vec<ExpectedCall> = json["expect"]["toolCalls"]
        .as_array()
        .expect("expect.toolCalls 是数组")
        .iter()
        .map(|call| ExpectedCall {
            name: call["name"].as_str().expect("名字").to_string(),
            ok: call["ok"].as_bool().expect("ok 是布尔"),
            output: call["output"].as_str().expect("输出是字符串").to_string(),
        })
        .collect();
    let expect_inputs: Vec<Value> = json["expect"]["toolCalls"]
        .as_array()
        .expect("expect.toolCalls 是数组")
        .iter()
        .map(|call| call["input"].clone())
        .collect();
    let expect_file_keys: Vec<String> = json["expect"]["files"]
        .as_object()
        .expect("expect.files 是对象")
        .keys()
        .cloned()
        .collect();
    let expect_files: BTreeMap<String, String> = json["expect"]["files"]
        .as_object()
        .expect("expect.files 是对象")
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| panic!("expect.files[{key}] 是内容 sha256 字符串"))
                    .to_string(),
            )
        })
        .collect();

    // turn 切分：首个 turn = 总条数 − 后续 turn 的条数之和。
    let follow_ups: Vec<usize> = json["followUpTurns"]
        .as_array()
        .map(|turns| {
            turns
                .iter()
                .map(|turn| turn["calls"].as_u64().expect("calls") as usize)
                .collect()
        })
        .unwrap_or_default();
    let mut turn_sizes = Vec::new();
    if !follow_ups.is_empty() {
        let tail: usize = follow_ups.iter().sum();
        turn_sizes.push(calls.len().saturating_sub(tail));
        turn_sizes.extend(follow_ups);
    } else if !calls.is_empty() {
        turn_sizes.push(calls.len());
    }

    // 结果对象：按 scheduled 首次出现顺序对齐（与第一层的折叠口径一致）。
    let mut expect_result_objects: Vec<Option<ResultObject>> = vec![None; expect_calls.len()];
    let mut order: Vec<String> = Vec::new();
    let mut index_of: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for event in json["expect"]["events"].as_array().expect("events") {
        if event["type"] != "tool.updated" {
            continue;
        }
        let payload = &event["payload"];
        let id = payload["toolCallId"].as_str().unwrap_or("").to_string();
        match payload["kind"].as_str() {
            Some("scheduled") => {
                index_of.insert(id.clone(), order.len());
                order.push(id);
            }
            Some("result") => {
                if let Some(position) = index_of.get(&id) {
                    let result = &payload["result"];
                    expect_result_objects[*position] = Some(ResultObject {
                        truncated: result["truncated"].as_bool().expect("truncated"),
                        original_bytes: result["originalBytes"].as_u64().expect("originalBytes"),
                        returned_bytes: result["returnedBytes"].as_u64().expect("returnedBytes"),
                        budget_strategy: result["budgetStrategy"]
                            .as_str()
                            .expect("budgetStrategy")
                            .to_string(),
                    });
                }
            }
            _ => {}
        }
    }

    Fixture {
        case: case.to_string(),
        seed,
        calls,
        expect_calls,
        expect_inputs,
        expect_file_keys,
        expect_files,
        turn_sizes,
        expect_result_objects,
    }
}

impl Fixture {
    /// 按 turn 切成 `(起, 止)` 下标区间，与录制时的 `followUpTurns` 一致。
    ///
    /// 用**下标**而不是切片：调用方手里的是它自己构造的调用结构（`agent-core` 传的是
    /// `ToolCallRequest`），共用的是切分规则而不是元素类型。
    pub fn turn_ranges(&self, total: usize) -> Vec<(usize, usize)> {
        let mut ranges = Vec::new();
        let mut start = 0;
        for size in &self.turn_sizes {
            let end = (start + size).min(total);
            ranges.push((start, end));
            start = end;
        }
        if start < total {
            ranges.push((start, total));
        }
        if ranges.is_empty() {
            ranges.push((0, total));
        }
        ranges
    }
}

fn hex_to_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len() / 2)
        .map(|index| u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).expect("十六进制"))
        .collect()
}

/// 一次性临时工作目录（用例跑完即删；不为一条断言引 `tempfile`）。
pub struct Workspace {
    root: PathBuf,
}

/// 目录名逐实例唯一：用例并行跑，同名目录会被另一条用例的 `Drop` 删掉。
static WORKSPACE_SEQ: AtomicUsize = AtomicUsize::new(0);

impl Workspace {
    pub fn new(case: &str) -> Self {
        let sequence = WORKSPACE_SEQ.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "agent-runtime-parity-{case}-{}-{sequence}",
            std::process::id()
        ));
        // 上一次失败留下的目录要清掉，否则种子文件会串味。
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("建临时工作目录");
        Self { root }
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    /// 工作目录的字符串形式（工具上下文用它）。
    pub fn cwd(&self) -> String {
        self.root.to_string_lossy().to_string()
    }

    pub fn seed(&self, files: &[(String, Vec<u8>)]) {
        for (relative, bytes) in files {
            let target = self.root.join(relative);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).expect("建种子目录");
            }
            std::fs::write(&target, bytes).expect("写种子文件");
        }
    }

    /// 把夹具入参里的 `${cwd}` 换成**本次**的工作目录。
    ///
    /// 在**已解析的结构**上替换（不经过 JSON 文本）：录制侧 TS 的 `substituteCwd` 就是这么做的，
    /// 而把它做成「序列化 → 文本替换 → 反解析」会把 Windows 路径的反斜杠插进 JSON 字符串里，
    /// 直接变成非法转义。
    ///
    /// 用原生分隔符（不转成 `/`）：于是录到的入参是 `C:\…\ws/hello.txt` 这种「目录用反斜杠 +
    /// 剧本自带的斜杠」的混形，投影后才与 `<cwd>/hello.txt` 一致；换成纯 `/` 会让 `PathBuf`
    /// 保留正斜杠，目录错误的文案就与观察值对不上了。
    pub fn substitute(&self, input: &Value) -> Value {
        let cwd = self.cwd();
        substitute_value(input, &cwd)
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// 在结构上替换 `${cwd}`（只在字符串值里换，不碰键与其他类型）。
fn substitute_value(value: &Value, cwd: &str) -> Value {
    match value {
        Value::String(text) => Value::String(text.replace("${cwd}", cwd)),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| substitute_value(item, cwd))
                .collect(),
        ),
        Value::Object(entries) => Value::Object(
            entries
                .iter()
                .map(|(key, inner)| (key.clone(), substitute_value(inner, cwd)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// spec §3 的「进程内路径」与「id（值）」两条规则的 Rust 投影。
///
/// 只做两件事：把隔离根 / 工作目录 / 系统临时目录的前缀换成 `<data-root>` / `<cwd>` / `<tmp>`
/// （长前缀在前，短前缀不会切坏长路径），把平铺在文本里的 uuid 换成 `<id>`。
pub fn project(value: &str, workspace: &str) -> String {
    let mut result = value.to_string();
    let tmp = std::env::temp_dir();
    // (前缀, 记号)：工作目录的两种书写（原生 / 正斜杠）都映射到同一个记号。
    let workspace_native = workspace.to_string();
    let workspace_posix = workspace.replace('\\', "/");
    let tmp_native = tmp.to_string_lossy().to_string();
    let tmp_posix = tmp.to_string_lossy().replace('\\', "/");
    for (prefix, token) in [
        (workspace_native.clone(), "<cwd>"),
        (workspace_posix.clone(), "<cwd>"),
        (tmp_native.clone(), "<tmp>"),
        (tmp_posix.clone(), "<tmp>"),
    ] {
        if prefix.is_empty() {
            continue;
        }
        result = result.replace(&prefix, token);
        // JSON 文本里的转义形态：spec §3 明写「单反斜杠与 JSON 转义的双反斜杠两种形态都要换」。
        // 比较入参时值会先序列化成 JSON 文本（`C:\\…\\ws`），不换这一形态就一条都对不上。
        let escaped = prefix.replace('\\', "\\\\");
        if escaped != prefix {
            result = result.replace(&escaped, token);
        }
    }
    replace_shell_diagnostic_prefix(&replace_uuids(&result))
}

/// 把 `bash: line N:` / `/usr/bin/bash: line N:` / `C:\…\bash.exe: line N:` 折成
/// `<shell>: line <n>:`。
///
/// 这些差异来自**怎么启动 bash**（argv[0] 与 `-lc` 包装的行数），不是工具行为；
/// 与 `normalize.mts` 同名规则逐条对齐。
fn replace_shell_diagnostic_prefix(value: &str) -> String {
    value
        .lines()
        .map(|line| {
            let Some(bash_at) = line.find("bash") else {
                return line.to_string();
            };
            let after = &line[bash_at + "bash".len()..];
            let after = after.strip_prefix(".exe").unwrap_or(after);
            let Some(rest) = after.strip_prefix(": line ") else {
                return line.to_string();
            };
            let Some(colon) = rest.find(':') else {
                return line.to_string();
            };
            if !rest[..colon].chars().all(|c| c.is_ascii_digit()) {
                return line.to_string();
            }
            format!("<shell>: line <n>:{}", &rest[colon + 1..])
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 入参投影：序列化后过同一份规则。
pub fn project_input(input: &Value, workspace: &str) -> Value {
    let text = project(
        &serde_json::to_string(input).expect("入参可序列化"),
        workspace,
    );
    serde_json::from_str(&text).expect("投影后仍是合法 JSON")
}

fn replace_uuids(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    let mut out = String::with_capacity(value.len());
    let mut index = 0;
    while index < chars.len() {
        match uuid_len_at(&chars, index) {
            Some(length) => {
                out.push_str("<id>");
                index += length;
            }
            None => {
                out.push(chars[index]);
                index += 1;
            }
        }
    }
    out
}

/// 若从 `start` 起是一个 uuid（8-4-4-4-12），返回它的字符长度。
fn uuid_len_at(text: &[char], start: usize) -> Option<usize> {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    const LENGTH: usize = 36;
    if start + LENGTH > text.len() {
        return None;
    }
    let slice = &text[start..start + LENGTH];
    let mut cursor = 0;
    for (group_index, group_len) in GROUPS.iter().enumerate() {
        if group_index > 0 {
            if slice[cursor] != '-' {
                return None;
            }
            cursor += 1;
        }
        for _ in 0..*group_len {
            if !slice[cursor].is_ascii_hexdigit() {
                return None;
            }
            cursor += 1;
        }
    }
    Some(LENGTH)
}
