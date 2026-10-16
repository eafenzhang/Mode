//! `Read` 的对拍用例（**纯函数层**：给定 `args` → 输出）。
//!
//! 期望值**不是手写的**：全部来自行为夹具
//! `packages/desktop/tests/agent-runtime-oracle/fixtures/*.json` 的 `expect`（由 oracle 录制）。
//! 本文件做的就是 Ruling 4 说的那件事——把 Rust 侧结果投影成夹具里的同一形状，然后**精确相等**
//! 比较。投影实现在 `agent_tools::test_support` 里（两端共用一份，见该模块注释）。
//!
//! 与 `crates/agent-core/tests/read_only_parity.rs` 的分工：那边走**会话循环 + 事件投影**
//! （注册表、`tool.updated` 序列），这边走**工具本身**，不经循环。两边共读同一份录制结果。
//!
//! 单独跑：`node scripts/rust-cargo.ts test -p agent-tools`

use std::fs;

use agent_tools::test_support::{load_fixture, project, project_input, Workspace};
use agent_tools::{
    estimated_tokens, invoke_by_name, registry, ModeToolFamily, ToolContext,
    UNIMPLEMENTED_ERROR_TYPE,
};

/// 逐条夹具、逐个工具调用，与录制结果精确相等。
fn assert_fixture(case: &str) {
    let fixture = load_fixture(case);
    let workspace = Workspace::new(case);
    workspace.seed(&fixture.seed);
    let context = ToolContext {
        cwd: workspace.cwd(),
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

        let outcome = invoke_by_name(&call.name, &args, &context);
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
        };
        for call in &fixture.calls {
            let args = workspace.substitute(&call.input);
            let _ = invoke_by_name(&call.name, &args, &context);
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

/// 注册表的内容本身是观察结果：只读切片里**只有** `Read`；`Glob`/`Grep` 未注册。
///
/// 证据：探针 `absence` 组（`Tool not found: Glob` / `Tool not found: Grep`）与夹具
/// `read-absent-search-tools.json`。
#[test]
fn registry_contains_only_read_from_the_read_only_slice() {
    let specs = registry();
    let names: Vec<&str> = specs.iter().map(|spec| spec.name).collect();
    assert_eq!(names, vec!["Read"]);
    assert_eq!(specs[0].family, ModeToolFamily::FileRead);
    assert!(specs[0].read_only);
    // Glob / Grep 必须**不**在注册表里：它们没有注册，调用得到 tool_not_found。
    assert!(specs.iter().all(|spec| spec.name != "Glob"));
    assert!(specs.iter().all(|spec| spec.name != "Grep"));
}

/// 未注册的名字答复的形状与观察到的 `tool_not_found` 逐字一致。
#[test]
fn unknown_tool_names_answer_like_the_oracle() {
    let context = ToolContext {
        cwd: ".".to_string(),
    };
    for name in ["Glob", "Grep"] {
        let error = invoke_by_name(name, &serde_json::json!({}), &context)
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

/// 只读工具的成功结果**形状不变量**，逐条夹具逐条调用地验：
/// `success`（投影成 `ok`）为真时，`truncated` 为假（观察到的每个成功结果都是 `false`）、
/// `budgetStrategy` 为 `"truncate"`，且两个字节计数都等于正文的 UTF-8 字节数。
///
/// 这三条是**不变量**而不是逐例抄下来的数字：前两条来自探针原始输出（每个成功结果都如此），
/// 第三条由「含中文的那次观察 `originalBytes: 23` = 23 字节」定下。
#[test]
fn success_result_shape_invariants_hold_for_every_recorded_call() {
    let mut successes = 0;
    for case in [
        "read-single",
        "read-missing",
        "read-twice-one-turn",
        "read-offset-edges",
        "read-error-paths",
        "read-absent-search-tools",
    ] {
        let fixture = load_fixture(case);
        let workspace = Workspace::new(case);
        workspace.seed(&fixture.seed);
        let context = ToolContext {
            cwd: workspace.cwd(),
        };
        for call in &fixture.calls {
            let args = workspace.substitute(&call.input);
            let Ok(outcome) = invoke_by_name(&call.name, &args, &context) else {
                continue;
            };
            successes += 1;
            assert!(!outcome.truncated, "{case}：成功结果不该是 truncated");
            assert_eq!(outcome.budget_strategy, "truncate", "{case}");
            assert_eq!(
                outcome.original_bytes,
                outcome.content.len() as u64,
                "{case}：originalBytes 应等于正文的 UTF-8 字节数"
            );
            assert_eq!(outcome.returned_bytes, outcome.original_bytes, "{case}");
        }
    }
    assert!(successes > 0, "一条成功结果都没有，这条不变量没验到东西");
}
