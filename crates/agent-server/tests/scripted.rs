//! 脚本化 provider 装配源的单元测试（Task 10 三层对拍的地基）。
//!
//! 覆盖：夹具解析（text / toolCalls 两类步 + 错误形状）、`${cwd}` 替换、
//! env 装配开关（无 env 不参与装配）。端到端（stdio 上以夹具回放三层）由
//! `packages/desktop/tests/agent-runtime-oracle/agent-server-replay.test.mts` 负责。

use std::path::PathBuf;

use agent_core::provider::{ModelRequest, ModelProvider};
use agent_server::{ProviderRequestContext, ProviderSource, ScriptedFixtureSource};
use serde_json::{json, Value};

fn write_fixture(name: &str, value: Value) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mode-agent-server-scripted-{}-{}",
        std::process::id(),
        name
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建临时目录");
    let path = dir.join("fixture.json");
    std::fs::write(&path, serde_json::to_string_pretty(&value).expect("序列化"))
        .expect("写夹具");
    path
}

fn context_with_cwd(cwd: &str) -> Option<ProviderRequestContext> {
    Some(ProviderRequestContext {
        session_id: "sess_x".to_string(),
        workspace_path: cwd.to_string(),
        workspace_identity: None,
    })
}

async fn stream_text(provider: &dyn ModelProvider, assistant_count: usize) -> String {
    let messages = (0..assistant_count)
        .map(|_| agent_core::provider::ModelMessage::Assistant {
            text: Some(format!("a{assistant_count}")),
            tool_calls: Vec::new(),
        })
        .collect();
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let request = ModelRequest {
        messages,
        tools: Vec::new(),
    };
    let response = provider
        .stream(request, sender)
        .await
        .expect("stream 成功");
    let mut text = String::new();
    while let Ok(delta) = receiver.try_recv() {
        if let agent_core::provider::ModelDelta::Text { text: chunk } = delta {
            text.push_str(&chunk);
        }
    }
    assert_eq!(response.stop_reason, "stop");
    text
}

#[tokio::test]
async fn parses_text_and_tool_call_steps_and_substitutes_cwd() {
    let path = write_fixture(
        "parse",
        json!({
            "case": "fixture-x",
            "prompt": "p",
            "script": [
                { "toolCalls": [
                    { "name": "Read", "input": { "file_path": "${cwd}/hello.txt" } },
                    { "name": "Bash", "input": { "command": "echo hi" } }
                ] },
                { "text": "answer ${cwd}" }
            ],
            "expect": { "toolCalls": [], "files": {}, "events": [] }
        }),
    );
    let source = ScriptedFixtureSource::from_fixture_path(path);
    let cwd = "D:\\tmp\\ws";
    let provider = source
        .resolve("oracle-mock", "mock-1", context_with_cwd(cwd).as_ref())
        .expect("resolve 成功");

    // 第 0 步（0 条 assistant 消息）→ 工具调用步：流里出现两个工具调用，
    // 入参里 `${cwd}` 已替换。
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let request = ModelRequest {
        messages: vec![agent_core::provider::ModelMessage::User("p".to_string())],
        tools: Vec::new(),
    };
    let response = provider.stream(request, sender).await.expect("stream 成功");
    assert_eq!(response.stop_reason, "tool-calls");
    let mut tool_inputs = Vec::new();
    while let Ok(delta) = receiver.try_recv() {
        if let agent_core::provider::ModelDelta::ToolCall { input, .. } = delta {
            tool_inputs.push(input);
        }
    }
    assert_eq!(tool_inputs.len(), 2);
    assert_eq!(tool_inputs[0]["file_path"], json!(format!("{cwd}/hello.txt")));
    assert_eq!(tool_inputs[1]["command"], json!("echo hi"));

    // 第 1 步（1 条 assistant 消息）→ 文本步，占位符已替换。
    let text = stream_text(provider.as_ref(), 1).await;
    assert_eq!(text, format!("answer {cwd}"));
}

#[tokio::test]
async fn out_of_range_script_repeats_last_step() {
    let path = write_fixture(
        "repeat",
        json!({ "script": [{ "text": "only" }] }),
    );
    let source = ScriptedFixtureSource::from_fixture_path(path);
    let provider = source
        .resolve("oracle-mock", "mock-1", context_with_cwd("D:\\tmp").as_ref())
        .expect("resolve 成功");
    // 下标越界（3 条 assistant 消息）→ 重复最后一步（与 TS mock 同口径）。
    let text = stream_text(provider.as_ref(), 3).await;
    assert_eq!(text, "only");
}

#[tokio::test]
async fn missing_file_surfaces_at_resolve_not_construction() {
    let source = ScriptedFixtureSource::from_fixture_path(
        std::env::temp_dir().join("mode-agent-server-scripted-does-not-exist.json"),
    );
    // `expect_err` 需要 Ok 侧实现 Debug（`Arc<dyn ModelProvider>` 没有），用 match。
    let error = match source.resolve("p", "m", context_with_cwd("D:\\tmp").as_ref()) {
        Ok(_) => panic!("缺失夹具应显式报错"),
        Err(error) => error,
    };
    assert!(error.contains("scripted provider 夹具不可读"), "实际: {error}");
}

#[tokio::test]
async fn malformed_steps_fail_with_stable_messages() {
    for (name, fixture, needle) in [
        ("no-script", json!({ "case": "x" }), "缺少 script 字段"),
        ("bad-step", json!({ "script": [{ "other": 1 }] }), "既没有 text"),
    ] {
        let path = write_fixture(name, fixture);
        let source = ScriptedFixtureSource::from_fixture_path(path);
        let error = match source.resolve("p", "m", context_with_cwd("D:\\tmp").as_ref()) {
            Ok(_) => panic!("{name}: 形状不合法应显式报错"),
            Err(error) => error,
        };
        assert!(error.contains(needle), "{name}: 实际 {error}");
    }
}

#[tokio::test]
async fn env_without_value_is_none() {
    // 临时改 env 有并发风险（其他测试并行读 env），只在「未设置」分支上断言：
    // 本测试进程从未设置该变量（CI/本地都不设），from_env 必须返回 None。
    if std::env::var(agent_server::SCRIPTED_PROVIDER_ENV).is_ok() {
        return; // 环境已设置时跳过（不与外部环境较劲）。
    }
    assert!(ScriptedFixtureSource::from_env().is_none());
}
