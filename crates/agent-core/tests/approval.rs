//! M2 Task 3C：审批面镜像与 `Ruleset` 累积的往返/行为用例。
//!
//! 期望值全部来自黑盒观察（`docs/specs/tauri-agent-runtime.md` §3.3，`probe-write.mts perm`
//! 于 2026-10-17 实跑），不是照着旧实现写的。
//!
//! 单独跑：`node scripts/rust-cargo.ts test -p agent-core --test approval`

use agent_core::approval::{Ruleset, RULESET_VERSION};
use agent_protocol::permission::{
    PermissionDecision, PermissionOptionKind, PermissionRequest, PermissionResponse,
    PermissionRuleValue, PermissionUpdate, PermissionUpdateType, RuleBehavior,
};
use serde_json::{json, Value};

const RULE_PATH: &str = "C:\\Users\\ADMINI~1\\AppData\\Local\\Temp\\mode-probe-ws-UXbSvF/perm.txt";

/// 2026-10-17 `probe-write.mts perm` 录到的请求，原样（仅把随机 uuid/临时目录换成固定值）。
fn observed_request() -> Value {
    json!({
        "input": {
            "file_path": RULE_PATH,
            "content": "x\n"
        },
        "reason": "Tool has side effects and requires approval",
        "requestId": "perm_958d9721-17bb-4a7e-90c1-dfd79eb2e581",
        "riskLevel": "medium",
        "sessionId": "sess_b06f9fe9-d3b5-485a-9fa9-0a2b15aad13d",
        "options": [
            {
                "kind": "allow_once",
                "name": "Allow once",
                "optionId": "allow_once",
                "response": {
                    "decision": "allow",
                    "reason": "Approved once"
                }
            },
            {
                "description": "Do not ask again for matching requests in this project",
                "kind": "allow_always",
                "name": "Always allow in this project",
                "optionId": "allow_project",
                "response": {
                    "decision": "allow",
                    "permissionUpdates": [
                        {
                            "behavior": "allow",
                            "rules": [
                                {
                                    "toolName": "Write",
                                    "ruleContent": RULE_PATH
                                }
                            ],
                            "type": "addRules"
                        }
                    ],
                    "reason": "Approved for this project"
                }
            },
            {
                "kind": "deny",
                "name": "Deny",
                "optionId": "deny",
                "response": {
                    "decision": "deny",
                    "reason": "The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). STOP what you are doing and wait for the user to tell you how to proceed."
                }
            }
        ],
        "toolCallId": "toolu_mock_0_0",
        "toolName": "Write",
        "turnId": "turn_d583f419-1354-4bd8-b6f8-bd8eb116f4b8"
    })
}

fn rule(tool: &str, path: &str) -> PermissionRuleValue {
    PermissionRuleValue {
        tool_name: tool.to_string(),
        rule_content: path.to_string(),
        extra: Default::default(),
    }
}

fn add_rules(behavior: RuleBehavior, rules: Vec<PermissionRuleValue>) -> PermissionUpdate {
    PermissionUpdate {
        update_type: PermissionUpdateType::AddRules,
        behavior,
        rules,
        extra: Default::default(),
    }
}

#[test]
fn permission_request_round_trips_the_observed_shape() {
    let value = observed_request();
    let request: PermissionRequest =
        serde_json::from_value(value.clone()).expect("观察到的请求形状必须能解进镜像类型");

    assert_eq!(request.tool_name, "Write");
    assert_eq!(request.risk_level, "medium");
    assert_eq!(
        request.reason,
        "Tool has side effects and requires approval"
    );
    assert_eq!(request.input["file_path"], RULE_PATH);

    let encoded = serde_json::to_value(&request).expect("镜像类型必须能序列化");
    assert_eq!(encoded, value, "往返必须逐字段一致");
}

#[test]
fn the_three_options_match_the_observed_table() {
    let request: PermissionRequest =
        serde_json::from_value(observed_request()).expect("请求能解码");

    let options = &request.options;
    assert_eq!(options.len(), 3, "观察到的选项恰好三条，不是四种决策");

    assert_eq!(options[0].option_id, "allow_once");
    assert_eq!(options[0].kind, PermissionOptionKind::AllowOnce);
    assert_eq!(options[0].name, "Allow once");
    assert_eq!(options[0].description, None);
    assert_eq!(options[0].response.decision, PermissionDecision::Allow);
    assert_eq!(options[0].response.permission_updates, None);

    assert_eq!(options[1].option_id, "allow_project");
    assert_eq!(options[1].kind, PermissionOptionKind::AllowAlways);
    assert_eq!(options[1].name, "Always allow in this project");
    assert_eq!(
        options[1].description.as_deref(),
        Some("Do not ask again for matching requests in this project")
    );
    let updates = options[1]
        .response
        .permission_updates
        .as_ref()
        .expect("allow_project 必须带 permissionUpdates");
    assert_eq!(updates.len(), 1, "观察里一次勾选恰好一条 addRules");
    assert_eq!(updates[0].update_type, PermissionUpdateType::AddRules);
    assert_eq!(updates[0].behavior, RuleBehavior::Allow);
    assert_eq!(
        updates[0].rules,
        vec![rule("Write", RULE_PATH)],
        "ruleContent 是原始大小写路径"
    );

    assert_eq!(options[2].option_id, "deny");
    assert_eq!(options[2].kind, PermissionOptionKind::Deny);
    assert_eq!(options[2].response.decision, PermissionDecision::Deny);
    assert_eq!(options[2].response.permission_updates, None);
}

#[test]
fn ruleset_value_is_the_observed_json() {
    let observed = json!({
        "version": 1,
        "allow": [
            { "toolName": "Write", "ruleContent": RULE_PATH }
        ]
    });
    let ruleset: Ruleset = serde_json::from_value(observed.clone()).expect("落盘值能解码");

    assert_eq!(ruleset.version, RULESET_VERSION);
    assert!(ruleset.deny.is_empty(), "观察里 deny 不落盘");

    let encoded = serde_json::to_value(&ruleset).expect("规则集能序列化");
    assert_eq!(encoded, observed, "写回的 value 必须与观察到的逐字一致");
    assert!(
        encoded.get("deny").is_none(),
        "空的 deny 不得出现在 JSON 里"
    );
}

#[test]
fn apply_updates_appends_allow_rules_in_order() {
    let mut ruleset = Ruleset::new();
    ruleset.apply_updates(&[add_rules(
        RuleBehavior::Allow,
        vec![rule("Write", "C:\\ws\\a.txt")],
    )]);
    ruleset.apply_updates(&[add_rules(
        RuleBehavior::Allow,
        vec![rule("Write", "C:\\ws\\b.txt")],
    )]);

    assert_eq!(
        ruleset.allow,
        vec![
            rule("Write", "C:\\ws\\a.txt"),
            rule("Write", "C:\\ws\\b.txt")
        ],
        "多条 allow_project 累积进同一 allow[]，顺序按到达顺序"
    );
    assert!(ruleset.deny.is_empty());
}

#[test]
fn apply_updates_routes_deny_rules_to_deny() {
    let mut ruleset = Ruleset::new();
    ruleset.apply_updates(&[add_rules(
        RuleBehavior::Deny,
        vec![rule("Write", "C:\\ws\\denied.txt")],
    )]);

    assert!(ruleset.allow.is_empty());
    assert_eq!(ruleset.deny, vec![rule("Write", "C:\\ws\\denied.txt")]);

    let encoded = serde_json::to_value(&ruleset).expect("规则集能序列化");
    assert_eq!(
        encoded["deny"][0]["ruleContent"],
        json!("C:\\ws\\denied.txt"),
        "非空 deny 才序列化"
    );
}

#[test]
fn duplicate_rules_are_appended_not_deduped() {
    // oracle 不可达：同路径第二次 allow_project 不会再发审批请求（spec §3.3「累积的边界」）。
    // 因此这里按 spec 的明确要求做纯追加，不把「去重」这个猜测写成规格。
    let update = add_rules(RuleBehavior::Allow, vec![rule("Write", "C:\\ws\\same.txt")]);
    let mut ruleset = Ruleset::new();
    ruleset.apply_updates(std::slice::from_ref(&update));
    ruleset.apply_updates(&[update]);

    assert_eq!(ruleset.allow.len(), 2);
}

#[test]
fn optional_response_fields_round_trip() {
    let response = PermissionResponse {
        decision: PermissionDecision::Allow,
        reason: Some("Approved once".to_string()),
        modified_input: Some(json!({ "file_path": "C:\\ws\\edited.txt" })),
        permission_updates: None,
        extra: Default::default(),
    };
    let encoded = serde_json::to_value(&response).expect("应答能序列化");
    assert_eq!(
        encoded,
        json!({
            "decision": "allow",
            "reason": "Approved once",
            "modifiedInput": { "file_path": "C:\\ws\\edited.txt" }
        })
    );

    let decoded: PermissionResponse = serde_json::from_value(encoded).expect("应答能解码");
    assert_eq!(decoded, response);
}

#[test]
fn unknown_discriminants_are_rejected() {
    let unknown_kind = json!({
        "input": {},
        "reason": "x",
        "requestId": "perm_1",
        "riskLevel": "medium",
        "sessionId": "sess_1",
        "options": [{
            "optionId": "allow_all",
            "kind": "allow_everything",
            "name": "Allow everything",
            "response": { "decision": "allow" }
        }],
        "toolCallId": "call_1",
        "toolName": "Write",
        "turnId": "turn_1"
    });
    assert!(
        serde_json::from_value::<PermissionRequest>(unknown_kind).is_err(),
        "未知 option kind 必须报错，不能静默当成已知选项"
    );

    let unknown_update = json!({
        "type": "removeRules",
        "behavior": "allow",
        "rules": []
    });
    assert!(
        serde_json::from_value::<PermissionUpdate>(unknown_update).is_err(),
        "未知 update type 必须报错"
    );

    let unknown_behavior = json!({
        "type": "addRules",
        "behavior": "ask",
        "rules": []
    });
    assert!(
        serde_json::from_value::<PermissionUpdate>(unknown_behavior).is_err(),
        "未知 behavior 必须报错"
    );
}

#[test]
fn ruleset_preserves_unobserved_fields() {
    // 持久化行可能先于 Rust 侧扩展；extra 保证读取时不丢字段（写回也保留）。
    let observed = json!({
        "version": 1,
        "allow": [],
        "futureField": { "anything": true }
    });
    let ruleset: Ruleset = serde_json::from_value(observed.clone()).expect("未知字段不阻断解码");
    assert_eq!(
        serde_json::to_value(&ruleset).expect("能序列化"),
        observed,
        "未知字段必须原样保留"
    );
}
