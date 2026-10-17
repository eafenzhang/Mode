//! `Read` 的对拍用例（**纯函数层**：给定 `args` → 输出）。
//!
//! 期望值**不是手写的**：全部来自行为夹具
//! `packages/desktop/tests/agent-runtime-oracle/fixtures/*.json` 的 `expect`（由 oracle 录制）。
//! 本文件做的就是 Ruling 4 说的那件事——把 Rust 侧结果投影成夹具里的同一形状，然后**精确相等**
//! 比较。投影实现在 `agent_tools::test_support` 里（两端共用一份，见该模块注释）。
//!
//! 与 `crates/agent-core/tests/read_only_parity.rs` 的分工：那边走**会话循环 + 事件投影**
//! （注册表、`tool.updated` 序列、**会话级的重复读去重**），这边走**工具本身**，不经循环。
//! 所以「需要会话状态」的夹具只在那边跑（见 `SESSION_HISTORY_CASES`）。
//!
//! 单独跑：`node scripts/rust-cargo.ts test -p agent-tools`

use std::fs;
use std::time::{Duration, SystemTime};

use agent_tools::read::TEXT_EXTENSIONS;
use agent_tools::test_support::{load_fixture, project, project_input, Workspace};
use agent_tools::{
    estimated_tokens, invoke_by_name, registry, FileState, ModeToolFamily, ToolContext,
    UNIMPLEMENTED_ERROR_TYPE,
};

/// 逐条夹具、逐个工具调用，与录制结果精确相等。
fn assert_fixture(case: &str) {
    let fixture = load_fixture(case);
    let workspace = Workspace::new(case);
    workspace.seed(&fixture.seed);
    let context = ToolContext {
        cwd: workspace.cwd(),
        artifact: None,
    };

    assert_eq!(
        fixture.expect_calls.len(),
        fixture.calls.len(),
        "{}：剧本的工具调用条数与录制的条数不一致",
        fixture.case
    );

    for (index, call) in fixture.calls.iter().enumerate() {
        let args = workspace.substitute(&call.input);
        // 入参也要对上：`${cwd}` 替换与 spec §3 的路径规则漂了的时候，这里比工具输出更早红。
        assert_eq!(
            project_input(&args, &context.cwd),
            fixture.expect_inputs[index],
            "{}：第 {index} 个调用的入参投影与录制不一致",
            fixture.case
        );

        let outcome = invoke_by_name(&call.name, &args, &context, &FileState::new());
        let (ok, output) = match &outcome {
            Ok(result) => (true, project(&result.content, &context.cwd)),
            Err(error) => {
                // 未实现的面**绝不能**出现在对拍里（见 agent_tools::UNIMPLEMENTED_ERROR_TYPE）。
                assert_ne!(
                    error.error_type, UNIMPLEMENTED_ERROR_TYPE,
                    "{}：第 {index} 个调用落到 M2 未实现的面：{}",
                    fixture.case, error.message
                );
                (false, project(&error.message, &context.cwd))
            }
        };
        let expected = &fixture.expect_calls[index];
        assert_eq!(
            call.name, expected.name,
            "{}：第 {index} 个调用的工具名",
            fixture.case
        );
        assert_eq!(ok, expected.ok, "{}：第 {index} 个调用的成败", fixture.case);
        assert_eq!(
            output, expected.output,
            "{}：第 {index} 个调用的输出与录制不一致",
            fixture.case
        );
    }
}

#[test]
fn read_offset_edges_matches_recorded_oracle() {
    assert_fixture("read-offset-edges");
}

#[test]
fn read_error_paths_matches_recorded_oracle() {
    assert_fixture("read-error-paths");
}

#[test]
fn read_absent_search_tools_matches_recorded_oracle() {
    assert_fixture("read-absent-search-tools");
}

#[test]
fn read_single_matches_recorded_oracle() {
    assert_fixture("read-single");
}

#[test]
fn read_missing_matches_recorded_oracle() {
    assert_fixture("read-missing");
}

#[test]
fn read_twice_one_turn_matches_recorded_oracle() {
    assert_fixture("read-twice-one-turn");
}

/// 第二层（文件快照）：只读工具**不改工作目录**——录制时有哪些键，跑完就还得有那些键，内容逐字不变。
///
/// 夹具里存的是内容 sha256；这里断言的是更强也更直接的性质（键集相同 + 内容未变），
/// 内容没变则 sha256 必然相同，所以不必为一条断言引一个散列依赖。
#[test]
fn read_only_tools_leave_the_workspace_untouched() {
    for case in ["read-offset-edges", "read-error-paths"] {
        let fixture = load_fixture(case);
        let workspace = Workspace::new(case);
        workspace.seed(&fixture.seed);
        let context = ToolContext {
            cwd: workspace.cwd(),
            artifact: None,
        };
        for call in &fixture.calls {
            let args = workspace.substitute(&call.input);
            let _ = invoke_by_name(&call.name, &args, &context, &FileState::new());
        }

        let mut keys: Vec<String> = fixture
            .seed
            .iter()
            .map(|(relative, _)| relative.replace('\\', "/"))
            .collect();
        keys.sort();
        let mut expected = fixture.expect_file_keys.clone();
        expected.sort();
        assert_eq!(keys, expected, "{case}：种子键集与夹具的第二层投影不一致");

        for (relative, bytes) in &fixture.seed {
            let after = fs::read(workspace.path().join(relative)).expect("种子文件仍在");
            assert_eq!(&after, bytes, "{case}：只读工具改了 {relative} 的内容");
        }
    }
}

/// 注册表的内容本身是观察结果：`Read` / `Write` / `Edit` / `Bash` 各占一条；
/// `Glob`/`Grep`/`ApplyPatch` 未注册。
///
/// 证据：探针 `probe-tool-surface.mts`（`Read` 在声明里）与 `probe-write.mts`（`Write` 在声明里、
/// `ApplyPatch` 不在且调用得 `tool_not_found`）；夹具 `read-absent-search-tools.json` 与
/// `write-empty-args.json`（正对照：空入参拿 `inputSchema validation` 而不是 `tool_not_found`）。
#[test]
fn registry_contains_only_read_from_the_read_only_slice() {
    let specs = registry();
    let names: Vec<&str> = specs.iter().map(|spec| spec.name).collect();
    assert_eq!(names, vec!["Read", "Write", "Edit", "Bash"]);
    assert_eq!(specs[0].family, ModeToolFamily::FileRead);
    assert!(specs[0].read_only);
    // 两个写工具：`file-write` 档、非只读——`started` 事件是
    // `readOnly:false` / `sideEffectScope:"workspace"`（夹具 `write-new` 的录制）。
    for spec in &specs[1..3] {
        assert_eq!(spec.family, ModeToolFamily::FileWrite);
        assert!(!spec.read_only, "{} 不该是只读的", spec.name);
    }
    // `Bash`：`shell` 档，但观察到的 `started` 是 `readOnly:true` / `sideEffectScope:"none"`
    // 且没有发审批请求（spec §3.4）——按观察值登记，不按直觉推断。
    assert_eq!(specs[3].family, ModeToolFamily::Shell);
    assert!(specs[3].read_only);
    // 未注册的三个：Glob / Grep（Task 2 实测）与 ApplyPatch（Task 3 实测）都不在表里。
    for absent in ["Glob", "Grep", "ApplyPatch"] {
        assert!(
            specs.iter().all(|spec| spec.name != absent),
            "{absent} 不该在注册表里"
        );
    }
}

/// 未注册的名字答复的形状与观察到的 `tool_not_found` 逐字一致。
#[test]
fn unknown_tool_names_answer_like_the_oracle() {
    let context = ToolContext {
        cwd: ".".to_string(),
        artifact: None,
    };
    for name in ["Glob", "Grep"] {
        let error = invoke_by_name(name, &serde_json::json!({}), &context, &FileState::new())
            .expect_err("未注册的工具必须失败");
        assert_eq!(error.error_type, "tool_not_found");
        assert_eq!(error.message, format!("Tool not found: {name}"));
        assert_eq!(error.code, Some("TOOL_NOT_FOUND"));
        assert_eq!(error.detail, None);
    }
}

/// 声明的 `input_schema` 是合法 JSON，且是那条从 oracle 抄下来的声明。
#[test]
fn read_input_schema_matches_the_recorded_declaration() {
    let schema = agent_tools::read_input_schema();
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["required"], serde_json::json!(["file_path"]));
    assert_eq!(schema["properties"]["offset"]["minimum"], 0);
    assert_eq!(schema["properties"]["limit"]["exclusiveMinimum"], 0);
    assert_eq!(schema["properties"]["file_path"]["type"], "string");
}

/// token 估算的边界：把 `ceil(字节/3)` 与「严格大于 25000」钉死
/// （探针 `trunc` #1/#2/#3；`floor` 与 75001 那一点矛盾，所以是上取整）。
#[test]
fn token_estimate_boundaries_match_the_recorded_probe() {
    assert_eq!(estimated_tokens(74_999), 25_000);
    assert_eq!(estimated_tokens(75_000), 25_000);
    assert_eq!(estimated_tokens(75_001), 25_001);
    // 探针里被选中的那几个范围（错误文案里的 tokens 数就是它们的估算值）。
    assert_eq!(estimated_tokens(121_999), 40_667);
    assert_eq!(estimated_tokens(152_438), 50_813);
    assert_eq!(estimated_tokens(152_499), 50_833);
    assert_eq!(estimated_tokens(304_999), 101_667);
}

/// 观察到的文本扩展名（含 `.xyz` 这种「没专门观察过、但实测按文本读」的）都走文本路径，
/// 且**没有扩展名**的文件（`Makefile`）也是。
#[test]
fn observed_text_extensions_read_as_text() {
    let workspace = Workspace::new("text-extensions");
    let mut seeds: Vec<(String, Vec<u8>)> = TEXT_EXTENSIONS
        .iter()
        .map(|extension| {
            (
                format!("sample.{extension}"),
                b"hello
"
                .to_vec(),
            )
        })
        .collect();
    seeds.push((
        "Makefile".to_string(),
        b"hello
"
        .to_vec(),
    ));
    workspace.seed(&seeds);
    let context = ToolContext {
        cwd: workspace.cwd(),
        artifact: None,
    };
    for (relative, _) in &seeds {
        let args = serde_json::json!({ "file_path": format!("{}/{relative}", context.cwd) });
        let outcome = invoke_by_name("Read", &args, &context, &FileState::new())
            .unwrap_or_else(|error| panic!("{relative} 应被当文本读：{}", error.message));
        assert_eq!(
            outcome.content,
            "1	hello
2	",
            "{relative}"
        );
    }
}

/// 重复读去重的**判据是 mtime**（探针 `probe-read-history.mts` 的 dedup3 是观察出处）：
///
/// - 同一入参、mtime 未变 → 命中（给「Wasted call」填充语）；
/// - 只把 mtime 往后拨、内容一字未改 → **不命中**（也给正文）；
/// - 另一个范围是另一个键；只有**成功**的读才进历史（读不存在的文件不留记录）。
#[test]
fn repeat_reads_are_stubbed_by_mtime() {
    let workspace = Workspace::new("read-history");
    workspace.seed(&[(
        "a.txt".to_string(),
        b"1
2
3
"
        .to_vec(),
    )]);
    let context = ToolContext {
        cwd: workspace.cwd(),
        artifact: None,
    };
    let args = serde_json::json!({ "file_path": format!("{}/a.txt", context.cwd) });
    let mut history = FileState::new();

    assert!(
        history.stub_for(&args, &context).is_none(),
        "首次读不该被去重"
    );
    history.record(&args, &context);
    let stub = history
        .stub_for(&args, &context)
        .expect("同一文件同一范围的第二次读应当命中");
    assert_eq!(stub.content, agent_tools::WASTED_CALL);
    assert_eq!(stub.original_bytes, 95, "填充语的字节数也是观察值");

    // 只碰 mtime（内容一字未改）→ 不命中（dedup3 #3 的观察）。
    let file = fs::File::options()
        .write(true)
        .open(workspace.path().join("a.txt"))
        .expect("打开");
    file.set_modified(SystemTime::now() + Duration::from_secs(5))
        .expect("改 mtime");
    assert!(
        history.stub_for(&args, &context).is_none(),
        "mtime 变了就不该命中（实测：只碰 mtime 也给正文）"
    );

    let ranged = serde_json::json!({ "file_path": format!("{}/a.txt", context.cwd), "offset": 2 });
    assert!(
        history.stub_for(&ranged, &context).is_none(),
        "不同的范围是不同的键"
    );

    let missing = serde_json::json!({ "file_path": format!("{}/absent.txt", context.cwd) });
    history.record(&missing, &context);
    assert!(
        history.stub_for(&missing, &context).is_none(),
        "读不存在的文件不留记录（实测：错误不去重）"
    );
}

/// 未实现面的标记只留在**真的没观察齐**的地方：图片扩展名（探针 `ext` 组：`.png` 走图片路径）。
#[test]
fn unimplemented_marker_covers_the_image_path() {
    let workspace = Workspace::new("image-marker");
    workspace.seed(&[(
        "pic.png".to_string(),
        b"not really a png
"
        .to_vec(),
    )]);
    let context = ToolContext {
        cwd: workspace.cwd(),
        artifact: None,
    };
    let args = serde_json::json!({ "file_path": format!("{}/pic.png", context.cwd) });
    let error = invoke_by_name("Read", &args, &context, &FileState::new())
        .expect_err("图片扩展名走未实现的面");
    assert_eq!(error.error_type, UNIMPLEMENTED_ERROR_TYPE);
    assert!(
        error.detail.as_deref().unwrap_or("").contains("read_image"),
        "detail 应指明是图片面：{:?}",
        error.detail
    );
}

#[test]
fn read_truncation_errors_matches_recorded_oracle() {
    assert_fixture("read-truncation-errors");
}

#[test]
fn read_partial_view_matches_recorded_oracle() {
    assert_fixture("read-partial-view");
}

#[test]
fn read_extension_tables_matches_recorded_oracle() {
    assert_fixture("read-extension-tables");
}

/// 复评 Minor 7：成功结果的四个字段对着**录制的 `result` 对象**比，不是对着构造函数比。
///
/// 夹具的 `expect.events` 里带着 oracle 真正发出的 `result` 载荷（`truncated` / `originalBytes` /
/// `returnedBytes` / `budgetStrategy`），所以这四条断言能**证伪** `ToolOutcome` 的取值。
#[test]
fn success_result_fields_match_the_recorded_events() {
    let mut compared = 0;
    for case in ["read-single", "read-twice-one-turn", "read-offset-edges"] {
        let fixture = load_fixture(case);
        let workspace = Workspace::new(case);
        workspace.seed(&fixture.seed);
        let context = ToolContext {
            cwd: workspace.cwd(),
            artifact: None,
        };
        for (index, call) in fixture.calls.iter().enumerate() {
            let Some(recorded) = fixture.expect_result_objects[index].as_ref() else {
                continue;
            };
            let args = workspace.substitute(&call.input);
            let outcome = invoke_by_name(&call.name, &args, &context, &FileState::new())
                .unwrap_or_else(|error| {
                    panic!("{case}：第 {index} 个调用本来应该成功：{}", error.message)
                });
            assert_eq!(
                outcome.truncated, recorded.truncated,
                "{case} #{index} truncated"
            );
            assert_eq!(
                outcome.original_bytes, recorded.original_bytes,
                "{case} #{index} originalBytes"
            );
            assert_eq!(
                outcome.returned_bytes, recorded.returned_bytes,
                "{case} #{index} returnedBytes"
            );
            assert_eq!(
                outcome.budget_strategy,
                recorded.budget_strategy.as_str(),
                "{case} #{index} budgetStrategy"
            );
            compared += 1;
        }
    }
    assert!(
        compared >= 3,
        "只比了 {compared} 条成功结果，这条用例没验到东西"
    );
}
