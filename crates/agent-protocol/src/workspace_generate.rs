//! `workspace/generateText` 与 `provider/testModelConnectivity` 的严格镜像。
//!
//! 真源：`packages/shared/src/mode-protocol/index.ts` 的
//! `modeWorkspaceGenerateTextParamsSchema` / `ResultSchema` 与
//! `modeProviderTestModelConnectivityResultSchema`。只镜像 wire 形状；
//! AbortSignal、请求超时与取消 operation 由调用方编排。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::session::ModelSelection;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceModelToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

/// `modeWorkspaceModelMessageSchema`：按 `role` 的判别联合，未知字段拒绝。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase", deny_unknown_fields)]
pub enum WorkspaceModelMessage {
    System {
        content: String,
    },
    User {
        content: String,
    },
    #[serde(rename_all = "camelCase")]
    Assistant {
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_calls: Option<Vec<WorkspaceModelToolCall>>,
    },
    #[serde(rename_all = "camelCase")]
    Tool {
        content: String,
        tool_call_id: String,
        tool_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceModelTool {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: Value,
}

/// `modeWorkspaceGenerateTextParamsSchema`；`workspace` 由宿主用
/// `build_workspace_ref` 注入，因此这里保留为开放对象。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GenerateTextParams {
    pub workspace: Value,
    pub selection: ModelSelection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages: Option<Vec<WorkspaceModelMessage>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<WorkspaceModelTool>>,
    pub query_source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
}

impl GenerateTextParams {
    /// 协议 `refine`：`prompt` 或非空 `messages` 至少一个。
    pub fn validate(&self) -> Result<(), String> {
        let has_prompt = self
            .prompt
            .as_deref()
            .is_some_and(|prompt| !prompt.is_empty());
        let has_messages = self
            .messages
            .as_ref()
            .is_some_and(|messages| !messages.is_empty());
        if !has_prompt && !has_messages {
            return Err("prompt 或 messages 至少需要提供一个".to_owned());
        }
        if self.query_source.is_empty() {
            return Err("querySource 不能为空".to_owned());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GenerateTextUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_tool_use: Option<ServerToolUse>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServerToolUse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_search_requests: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_fetch_requests: Option<u64>,
}

/// `modeWorkspaceGenerateTextResultSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GenerateTextResult {
    pub text: String,
    pub selection: ModelSelection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<WorkspaceModelToolCall>>,
    /// 兼容仍在运行的旧 app-server；新 CLI 始终返回结构化结束原因。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<GenerateTextUsage>,
}

/// `modeWorkspaceCancelGenerateTextResultSchema`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CancelGenerateTextResult {
    pub operation_id: String,
    pub cancelled: bool,
}

/// `modeProviderTestModelConnectivityResultSchema`：成功恒为 `{ success: true }`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TestModelConnectivityResult {
    pub success: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection() -> ModelSelection {
        serde_json::from_value(serde_json::json!({
            "providerId": "p1",
            "modelId": "m1",
        }))
        .unwrap()
    }

    #[test]
    fn message_union_rejects_unknown_role_fields() {
        let message: WorkspaceModelMessage = serde_json::from_value(serde_json::json!({
            "role": "user",
            "content": "hi",
        }))
        .unwrap();
        assert_eq!(
            message,
            WorkspaceModelMessage::User {
                content: "hi".to_owned()
            }
        );
        let error = serde_json::from_value::<WorkspaceModelMessage>(serde_json::json!({
            "role": "user",
            "content": "hi",
            "extra": true,
        }))
        .unwrap_err();
        assert!(error.to_string().contains("unknown field"), "{error}");
    }

    #[test]
    fn params_require_prompt_or_messages() {
        let base = GenerateTextParams {
            workspace: serde_json::json!({ "workspacePath": "D:/ws" }),
            selection: selection(),
            prompt: None,
            messages: None,
            tools: None,
            query_source: "git-commit".to_owned(),
            max_output_tokens: None,
            operation_id: None,
        };
        assert!(base.validate().is_err());
        let mut with_prompt = base.clone();
        with_prompt.prompt = Some("hello".to_owned());
        assert!(with_prompt.validate().is_ok());
        let mut with_empty_messages = base.clone();
        with_empty_messages.messages = Some(Vec::new());
        assert!(with_empty_messages.validate().is_err());
        let mut with_empty_query = with_prompt.clone();
        with_empty_query.query_source = String::new();
        assert!(with_empty_query.validate().is_err());
    }

    #[test]
    fn result_round_trips_minimal_shape() {
        let result = GenerateTextResult {
            text: "done".to_owned(),
            selection: selection(),
            tool_calls: None,
            finish_reason: Some("stop".to_owned()),
            usage: Some(GenerateTextUsage {
                total_tokens: Some(3),
                ..GenerateTextUsage::default()
            }),
        };
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "text": "done",
                "selection": { "providerId": "p1", "modelId": "m1" },
                "finishReason": "stop",
                "usage": { "totalTokens": 3 },
            })
        );
        let decoded: GenerateTextResult = serde_json::from_value(value).unwrap();
        assert_eq!(decoded, result);
    }
}
