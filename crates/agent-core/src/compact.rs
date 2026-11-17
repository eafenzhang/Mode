//! C2 compact 的摘要提示、落库文本形状与模型响应校验。
//!
//! 口径：`docs/specs/agent-c2-contract.md` §1.3（摘要落库文本）、§1.4（compact 模型
//! 请求的用户消息形状）、§1.8（失败语义）；`docs/specs/agent-c2-context-memory.md` §3。
//!
//! Clean-room 纪律（C2 spec §10）：
//! - 摘要续接文本（[`summary_continuation`]）是**落库数据契约**——逐字取自机器录制的
//!   夹具 `expect` 与契约 §1.3 的观察值，不是提示词创作；
//! - compact 请求的用户消息（[`compact_prompt`]）是本仓库自撰的提示词，只保留契约
//!   §1.4 记录的接口性片段（`CRITICAL: Respond with TEXT ONLY…` 开头、
//!   `<analysis>/<summary>` 结构、`Additional Instructions:` / `REMINDER:` 尾部）。

/// 摘要消息 `summary.title` 的观察值（契约 §1.3）。
pub const SUMMARY_TITLE: &str = "Compact summary";

/// 摘要续接文本的固定前缀（契约 §1.3 逐字观察值；落库 `part.data.text` 的组成部分）。
const SUMMARY_PREFIX: &str = "This session is being continued from a previous conversation \
                               that ran out of context. The summary below covers the earlier \
                               portion of the conversation.\n\n";

/// 摘要续接文本的固定后缀（契约 §1.3 逐字观察值）。
const SUMMARY_SUFFIX: &str = "\nContinue the conversation from where it left off without \
                               asking the user any further questions. Resume directly — do not \
                               acknowledge the summary, do not recap what was happening, do not \
                               preface with \"I'll continue\" or similar. Pick up the last task \
                               as if the break never happened.";

/// 组装摘要消息的续接文本：固定前缀 + 模型摘要正文 + 固定后缀（契约 §1.3 观察形状）。
pub fn summary_continuation(summary: &str) -> String {
    format!("{SUMMARY_PREFIX}{summary}{SUMMARY_SUFFIX}")
}

/// compact 模型请求末尾的用户消息（契约 §1.4 的形状；正文由本仓库自撰）。
///
/// - 恒以 `CRITICAL: Respond with TEXT ONLY. Do NOT call any tools.` 开头；
/// - 要求 `<analysis>` / `<summary>` 两段输出；
/// - 带 `instructions` 时在尾部插入 `Additional Instructions:\n<instructions>`，
///   末尾统一是 `REMINDER: Do NOT call any tools. …`（有无 instructions 的两种尾部
///   结构与观察一致）。
pub fn compact_prompt(instructions: Option<&str>) -> String {
    let mut prompt = String::from(
        "CRITICAL: Respond with TEXT ONLY. Do NOT call any tools.\n\
         \n\
         Summarize the conversation above so that it can be continued in a fresh context.\n\
         Reply with exactly two blocks, in this order:\n\
         \n\
         <analysis>\n\
         Note the key facts, decisions, open questions, and any identifiers, file paths,\n\
         or command output that must stay verbatim.\n\
         </analysis>\n\
         <summary>\n\
         The summary the next context will read. Preserve identifiers and pending work\n\
         exactly; do not add commentary about summarizing.\n\
         </summary>\n",
    );
    if let Some(instructions) = instructions {
        prompt.push_str(&format!("\nAdditional Instructions:\n{instructions}\n"));
    }
    prompt.push_str(
        "\nREMINDER: Do NOT call any tools. Reply with the <analysis> and <summary> blocks \
         only.",
    );
    prompt
}

/// compact 模型响应的失败面（契约 §1.8 的两种观察失败）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactResponseError {
    /// provider 发起了工具调用（观察：`Tool use is not allowed during compaction`，
    /// `retryable=false`）。
    ToolUseNotAllowed,
    /// provider 没有产出摘要文本（观察：`Failed to generate compact summary`，
    /// `retryable=true`）。
    EmptySummary,
}

impl CompactResponseError {
    /// `turn.failed.error.message`（观察值逐字）。
    pub fn message(self) -> &'static str {
        match self {
            Self::ToolUseNotAllowed => "Tool use is not allowed during compaction",
            Self::EmptySummary => "Failed to generate compact summary",
        }
    }

    /// `turn.failed.error.attribution.retryable`（观察值）。
    pub fn retryable(self) -> bool {
        match self {
            Self::ToolUseNotAllowed => false,
            Self::EmptySummary => true,
        }
    }

    /// `turn.failed.error.code`（观察值）。
    pub fn code(self) -> &'static str {
        "MODEL_ERROR"
    }
}

/// 校验 compact 的模型响应：工具调用先于空文本判定（两者都存在时按工具调用报错）。
///
/// 成功返回**原样**的摘要正文（落库 `summary.body` 是模型返回文本，不 trim 改写）。
pub fn validate_compact_response(
    tool_call_count: usize,
    stop_reason: &str,
    text: &str,
) -> Result<String, CompactResponseError> {
    if tool_call_count > 0 || stop_reason == "tool-calls" {
        return Err(CompactResponseError::ToolUseNotAllowed);
    }
    if text.trim().is_empty() {
        return Err(CompactResponseError::EmptySummary);
    }
    Ok(text.to_string())
}
