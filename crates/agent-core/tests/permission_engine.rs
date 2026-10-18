//! 权限引擎的表驱动用例（M2 Task 6）。
//!
//! 全部期望值来自 `probe-permission.mts` 的两批命令电池（spec §3.3）：
//! `None` = 无审批；`Some("high")` / `Some("medium")` = 对应 riskLevel 的审批。

use serde_json::json;

use agent_core::approval::Ruleset;
use agent_core::permission::{evaluate, Decision, HIGH_RISK_REASON, SIDE_EFFECT_REASON};
use agent_core::{ModeToolFamily, ToolSpec};

fn spec(name: &'static str, family: ModeToolFamily) -> ToolSpec {
    ToolSpec {
        name: name.to_string(),
        family,
        schema: json!({}),
        read_only: matches!(family, ModeToolFamily::FileRead),
        plugin: false,
    }
}

fn bash(command: &str) -> Decision {
    evaluate(
        &spec("Bash", ModeToolFamily::Shell),
        &json!({ "command": command }),
        &Ruleset::new(),
    )
}

fn assert_risk(command: &str, expected: Option<&'static str>) {
    let decision = bash(command);
    match expected {
        None => assert!(decision.is_allow(), "{command} 应无审批，实际 {decision:?}"),
        Some(level) => assert_eq!(
            decision,
            Decision::Ask {
                risk_level: level,
                reason: HIGH_RISK_REASON,
            },
            "{command} 应为 {level} 审批"
        ),
    }
}

#[test]
fn bash_command_battery_matches_the_observed_matrix() {
    const ALLOWED: [&str; 45] = [
        "echo hi",
        "echo hi | cat",
        "git status",
        "git diff",
        "cat file1",
        "grep hi file1",
        "ls",
        "ls -la",
        "pwd",
        "head -1 file1",
        "tail -1 file1",
        "wc -l file1",
        "find . -name '*.txt'",
        "rg hi",
        "git log",
        "git show",
        "git branch",
        "git rev-parse HEAD",
        "which bash",
        "date",
        "uname",
        "whoami",
        "printf hi",
        "true",
        "sort file1",
        "uniq file1",
        "cut -d: -f1 file1",
        "test -f file1",
        "[ -f file1 ]",
        "echo \"hi\"",
        "echo 'hi'",
        "echo hi 2>/dev/null",
        "stat file1",
        "file file1",
        "du -sh .",
        "ps",
        "df -h",
        "grep -r hi .",
        "cat file1 | head -1",
        "pwd && echo hi",
        "echo hi; pwd",
        "echo hi || true",
        "echo hi | grep h",
        "echo hi && echo bye",
        "find . -name '*.txt' -print",
    ];
    for command in ALLOWED {
        assert_risk(command, None);
    }

    const HIGH: [&str; 32] = [
        "echo hi > out.txt",
        "echo hi >> out.txt",
        "echo $(echo hi)",
        "echo `echo hi`",
        "mkdir dir1",
        "touch file1",
        "cp file1 file2",
        "mv file2 file3",
        "rm file3",
        "rm -r dir1",
        "rm -rf dir1",
        "chmod 600 file1",
        "sed -i 's/a/b/' file1",
        "git reset --hard",
        "git push",
        "npm --version",
        "pnpm --version",
        "node -e \"console.log(1)\"",
        "python -c \"print(1)\"",
        "cmd /c echo hi",
        "curl -s http://127.0.0.1:9",
        "sudo true",
        "kill -0 99999",
        "find . -name '*.tmp' -delete",
        "echo hi | xargs echo",
        "eval \"echo hi\"",
        "bash -c 'echo hi'",
        "sh -c 'echo hi'",
        "env",
        "tee file4 <<< \"hi\"",
        "echo hi &",
        "echo $HOME",
    ];
    for command in HIGH {
        assert_risk(command, Some("high"));
    }
}

#[test]
fn write_and_edit_are_medium_unless_a_ruleset_matches() {
    let write = spec("Write", ModeToolFamily::FileWrite);
    let path = "C:/ws/a.txt";
    let args = json!({ "file_path": path, "content": "a\n" });
    assert_eq!(
        evaluate(&write, &args, &Ruleset::new()),
        Decision::Ask {
            risk_level: "medium",
            reason: SIDE_EFFECT_REASON,
        }
    );

    let ruleset: Ruleset = serde_json::from_value(json!({
        "version": 1,
        "allow": [{ "toolName": "Write", "ruleContent": path }]
    }))
    .expect("规则集能解码");
    assert_eq!(evaluate(&write, &args, &ruleset), Decision::Allow);
    assert_eq!(
        evaluate(&write, &json!({ "file_path": "C:/ws/b.txt" }), &ruleset),
        Decision::Ask {
            risk_level: "medium",
            reason: SIDE_EFFECT_REASON,
        }
    );
}

#[test]
fn read_and_task_control_need_no_approval() {
    let ruleset = Ruleset::new();
    assert!(evaluate(
        &spec("Read", ModeToolFamily::FileRead),
        &json!({ "file_path": "x" }),
        &ruleset
    )
    .is_allow());
    for name in ["TaskOutput", "TaskStop"] {
        assert!(
            evaluate(
                &spec(name, ModeToolFamily::TaskControl),
                &json!({}),
                &ruleset
            )
            .is_allow(),
            "{name} 观察为无审批"
        );
    }
}
