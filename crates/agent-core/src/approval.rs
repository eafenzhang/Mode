//! 审批规则集：`allow_project` 的 `permissionUpdates` 如何累积成持久化的 JSON 值。
//!
//! 观察出处：`docs/specs/tauri-agent-runtime.md` §3.3「落盘」——`value` 的逐字形状是
//! `{"version":1,"allow":[{"toolName":…,"ruleContent":…}]}`：只有 `allow[]`，`deny` 不落盘。
//!
//! # 为什么是「追加」而不是「去重」
//!
//! spec 的「累积的边界」记录了两条**探不到**的事实：一次应答恰好一条规则、重复规则在 oracle
//! 里不可达（同路径第二次 `allow_project` 根本不会再发审批请求）。因此「同一条规则要不要
//! 去重」**没有观察依据**；本实现按 spec 的明确要求做纯追加，不把猜测写成规格。
//!
//! 持久化归谁写（Rust 直连 sqlite / 经 Host 代理）仍是未裁决项 D1，本模块不碰 IO。
use serde::{Deserialize, Serialize};

use agent_protocol::permission::{PermissionRuleValue, PermissionUpdate, RuleBehavior};
use agent_protocol::ExtraFields;

/// 观察到的 `version` 值。
pub const RULESET_VERSION: u32 = 1;

/// 持久化在 `local_setting.value` 里的规则集（`namespace=permission`、`key=ruleset`）。
///
/// 字段顺序按观察到的 JSON 排列（`version` 在前、`allow` 在后）；`deny` 为空时**不序列化**，
/// 这样产出的值与该行逐字一致。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ruleset {
    pub version: u32,
    #[serde(default)]
    pub allow: Vec<PermissionRuleValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<PermissionRuleValue>,
    /// 观察之外的字段原样保留，避免一次协议扩展把这行读坏。
    #[serde(flatten)]
    pub extra: ExtraFields,
}

impl Ruleset {
    /// 空规则集（`{"version":1,"allow":[]}`）。
    pub fn new() -> Self {
        Self {
            version: RULESET_VERSION,
            allow: Vec::new(),
            deny: Vec::new(),
            extra: ExtraFields::new(),
        }
    }

    /// 把一条应答里的全部 `addRules` 按到达顺序追加进来。
    ///
    /// 不去重：重复规则的 oracle 不可达，去重会引入没有依据的行为（见模块注释）。
    pub fn apply_updates(&mut self, updates: &[PermissionUpdate]) {
        for update in updates {
            match update.behavior {
                RuleBehavior::Allow => self.allow.extend(update.rules.iter().cloned()),
                RuleBehavior::Deny => self.deny.extend(update.rules.iter().cloned()),
            }
        }
    }

    /// 非破坏式版本：返回应用后的新规则集。
    pub fn with_updates(&self, updates: &[PermissionUpdate]) -> Self {
        let mut next = self.clone();
        next.apply_updates(updates);
        next
    }

    /// 规则是否**精确命中**（`toolName` 与 `ruleContent` 都逐字相等）。
    ///
    /// 观察（spec §3.3）：`allow_project` 之后同一路径的第二次 Write 不再请求，
    /// 不同路径仍会请求；因此匹配是逐字相等，不做前缀/通配。
    /// `deny` 规则在观察里从未落盘，所以这里**只查 allow**，不发明 deny 优先级。
    pub fn allows(&self, tool_name: &str, rule_content: &str) -> bool {
        self.allow
            .iter()
            .any(|rule| rule.tool_name == tool_name && rule.rule_content == rule_content)
    }
}

impl Default for Ruleset {
    fn default() -> Self {
        Self::new()
    }
}
