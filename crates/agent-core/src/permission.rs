//! 权限引擎：工具 + 入参 + 规则集 → allow / ask（M2 Task 6）。
//!
//! 观察出处：`docs/specs/tauri-agent-runtime.md` §3.3（`probe-permission.mts` 的
//! 审批矩阵与两批 Bash 命令电池）。这里只实现**策略判定**，不实现交互协议
//! （`interaction/requestPermission` 的收发与 UI 归后续接线）。
//!
//! # 判定模型（全部来自观察）
//!
//! - `Read` / `TaskOutput` / `TaskStop`：无审批 → allow；
//! - `Write` / `Edit`：`medium` ask，除非 `Ruleset` 里有 `toolName + ruleContent` 的逐字规则；
//! - `Bash`：**默认 high**，只有命令落在观察到的安全 allowlist 里才 allow；
//! - 其余 family（本仓库还没注册的工具）：allow，等有观察再收紧。
//!
//! # 已知边界
//!
//! 安全 allowlist 是**观察枚举**，不是从实现里搬来的：未观察到的安全命令可能被多问一次
//! （保守方向）；`deny` 是用户答复，不是策略输出，所以 [`Decision`] 没有 Deny 变体。

use std::sync::mpsc::{self, Receiver, Sender};

use serde_json::Value;

use agent_protocol::permission::{PermissionDecision, PermissionRequest, PermissionResponse};
use agent_protocol::ExtraFields;
use agent_tools::{ModeToolFamily, ToolSpec};

use crate::approval::Ruleset;

/// 审批交互的消费端：会话循环拿到 [`PermissionRequest`] 后交给它，取回答复。
pub trait PermissionHandler {
    fn request(&mut self, request: &PermissionRequest) -> PermissionResponse;
}

/// 默认策略：一律 `allow_once`（等价于驱动对权限请求的默认应答）。
pub struct AutoAllow;

impl PermissionHandler for AutoAllow {
    fn request(&mut self, _request: &PermissionRequest) -> PermissionResponse {
        PermissionResponse {
            decision: PermissionDecision::Allow,
            reason: Some("Approved once".to_string()),
            modified_input: None,
            permission_updates: None,
            extra: ExtraFields::new(),
        }
    }
}

/// 跨线程的审批适配器：把请求交给宿主线程，阻塞等待应答。
///
/// 这是 stdio `interaction/requestPermission` 反向请求的「本地半边」：宿主线程从
/// [`Receiver<PermissionRequest>`] 取请求、转发给 host/UI，再把答复通过
/// [`Sender<PermissionResponse>`] 送回。通道断开时按 **deny** 处理（fail closed），
/// 不把「没人应答」当成允许。
pub struct ChannelPermissionHandler {
    requests: Sender<PermissionRequest>,
    responses: Receiver<PermissionResponse>,
}

impl ChannelPermissionHandler {
    /// 返回 `(handler, 请求接收端, 应答发送端)`。
    pub fn pair() -> (
        Self,
        Receiver<PermissionRequest>,
        Sender<PermissionResponse>,
    ) {
        let (requests, request_receiver) = mpsc::channel();
        let (response_sender, responses) = mpsc::channel();
        (
            Self {
                requests,
                responses,
            },
            request_receiver,
            response_sender,
        )
    }
}

impl PermissionHandler for ChannelPermissionHandler {
    fn request(&mut self, request: &PermissionRequest) -> PermissionResponse {
        if self.requests.send(request.clone()).is_err() {
            return denied("permission channel closed");
        }
        self.responses
            .recv()
            .unwrap_or_else(|_| denied("permission channel closed"))
    }
}

fn denied(reason: &str) -> PermissionResponse {
    PermissionResponse {
        decision: PermissionDecision::Deny,
        reason: Some(reason.to_string()),
        modified_input: None,
        permission_updates: None,
        extra: ExtraFields::new(),
    }
}

/// 权限判定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Ask {
        risk_level: &'static str,
        reason: &'static str,
    },
}

impl Decision {
    pub fn is_allow(&self) -> bool {
        matches!(self, Self::Allow)
    }
}

/// `Write` / `Edit` 的审批文案（逐字观察）。
pub const SIDE_EFFECT_REASON: &str = "Tool has side effects and requires approval";
/// `deny` 选项的答复文案（逐字观察）。
pub const DENY_REASON: &str = "The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). STOP what you are doing and wait for the user to tell you how to proceed.";
/// Bash 高危命令的审批文案（逐字观察）。
pub const HIGH_RISK_REASON: &str = "High risk tools require explicit approval";

/// 观察到的安全命令（Bash 电池 1/2 里 `none` 的那些）。
///
/// 刻意**不**包含 `sed` / `sh` / `bash` / `awk`：`sed -i`、`sh -c`、`bash -c`、`awk` 都观察到 high；
/// 安全的子形态无法从观察里区分，因此整类保守地按 high 处理。
const SAFE_COMMANDS: [&str; 27] = [
    "ls", "pwd", "head", "tail", "wc", "find", "rg", "git", "which", "date", "uname", "whoami",
    "printf", "true", "sort", "uniq", "cut", "test", "[", "echo", "cat", "grep", "stat", "file",
    "du", "ps", "df",
];

/// `git` 的安全子命令（观察：status/diff/log/show/branch/rev-parse 为 none；reset/push 为 high）。
const SAFE_GIT_SUBCOMMANDS: [&str; 6] = ["status", "diff", "log", "show", "branch", "rev-parse"];

/// 判定一次工具调用。
pub fn evaluate(tool: &ToolSpec, args: &Value, ruleset: &Ruleset) -> Decision {
    match tool.family {
        ModeToolFamily::FileRead => Decision::Allow,
        ModeToolFamily::FileWrite => {
            let path = args.get("file_path").and_then(Value::as_str);
            if path.is_some_and(|path| ruleset.allows(tool.name, path)) {
                return Decision::Allow;
            }
            Decision::Ask {
                risk_level: "medium",
                reason: SIDE_EFFECT_REASON,
            }
        }
        ModeToolFamily::Shell => {
            let command = args
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if ruleset.allows("Bash", command) || is_safe_command(command) {
                return Decision::Allow;
            }
            Decision::Ask {
                risk_level: "high",
                reason: HIGH_RISK_REASON,
            }
        }
        // 观察：TaskOutput / TaskStop 都没有审批请求。
        ModeToolFamily::TaskControl => Decision::Allow,
        // 本仓库尚未注册其它 family；有观察后再收紧，不按直觉给 ask。
        _ => Decision::Allow,
    }
}

/// 命令是否落在观察到的安全集合里。
///
/// 规则（全部由电池观察支撑）：
/// - `>` / `>>` / `$` / 反引号 / 单个 `&` → 不安全；
/// - `2>/dev/null` 是例外（观察为 none），先剔除；
/// - 按 `|` / `;` / `&&` / `||` 拆段后，每段第一个词必须在 [`SAFE_COMMANDS`]；
/// - `git` 的第二个词必须在 [`SAFE_GIT_SUBCOMMANDS`]；
/// - `find` 带 `-delete` / `-exec` / `-ok` → 不安全。
fn is_safe_command(command: &str) -> bool {
    let normalized = command.replace("2>/dev/null", "");
    if normalized.contains('>')
        || normalized.contains('$')
        || normalized.contains('`')
        || has_background_ampersand(&normalized)
    {
        return false;
    }

    for segment in normalized.split(['|', ';']) {
        for part in segment.split("&&") {
            let tokens: Vec<&str> = part.split_whitespace().collect();
            let Some(name) = tokens.first() else {
                continue;
            };
            if *name == "git" {
                let subcommand = tokens.get(1).copied().unwrap_or_default();
                if !SAFE_GIT_SUBCOMMANDS.contains(&subcommand) {
                    return false;
                }
                continue;
            }
            if *name == "find"
                && tokens.iter().any(|token| {
                    token.starts_with("-delete")
                        || token.starts_with("-exec")
                        || token.starts_with("-ok")
                })
            {
                return false;
            }
            if !SAFE_COMMANDS.contains(name) {
                return false;
            }
        }
    }
    true
}

/// 是否存在**后台 `&`**（`&&` 不算）。
fn has_background_ampersand(command: &str) -> bool {
    let bytes = command.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'&' {
            if index + 1 < bytes.len() && bytes[index + 1] == b'&' {
                index += 2;
                continue;
            }
            return true;
        }
        index += 1;
    }
    false
}
