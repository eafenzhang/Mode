//! `mcp/list` facade：workspace MCP 状态。
//!
//! 行为真源：`packages/services/src/mode-agent/modeAgentService.ts` 的 `listMcpServerStatuses`。
//!
//! 差异：TS 使用独立的 mcp-status 泳道 client；本 facade 复用 workspace runtime。
//! 兼容重试已对齐：旧 Agent 不认识 `mode` / `mcpServers` 时按 `unrecognized_keys`
//! 省略对应字段重试；`mode: "status"` 被拒时返回稳定错误 `McpStatusModeUnsupported`，
//! 不做会改变连接集合的降级重试。

use agent_protocol::mcp::{McpListMode, McpListParams, McpListResult, MCP_LIST_METHOD};
use agent_protocol::session_create::McpServer;
use mode_protocol_client::ClientError;
use serde_json::Value;

use crate::control::{AgentControlClient, AgentWorkspaceTarget, ControlError, RuntimePolicy};
use crate::session_read::session_workspace_ref;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListMcpServersOptions {
    pub mcp_servers: Option<Vec<McpServer>>,
    pub mode: Option<McpListMode>,
}

impl AgentControlClient {
    /// `mcp/list`：返回按 serverName 索引的状态快照。
    pub async fn list_mcp_server_statuses(
        &self,
        target: &AgentWorkspaceTarget,
        options: ListMcpServersOptions,
    ) -> Result<McpListResult, ControlError> {
        let process = self
            .client_for(target, RuntimePolicy::StartIfNeeded)
            .await?;
        request_mcp_list(&process, target, &options).await
    }

    /// `listMcpServerStatuses` 的完整兼容路径：按旧 Agent 的 `unrecognized_keys`
    /// 省略 `mode` / `mcpServers` 后重试一次。
    pub async fn list_mcp_server_statuses_compat(
        &self,
        target: &AgentWorkspaceTarget,
        options: ListMcpServersOptions,
    ) -> Result<McpListResult, ControlError> {
        let process = self
            .client_for(target, RuntimePolicy::StartIfNeeded)
            .await?;
        match request_mcp_list(&process, target, &options).await {
            Ok(result) => Ok(result),
            Err(ControlError::Client(error)) => {
                let keys = unrecognized_top_level_keys(&error);
                let omit_mode = options.mode.is_some() && keys.iter().any(|key| key == "mode");
                let omit_servers =
                    options.mcp_servers.is_some() && keys.iter().any(|key| key == "mcpServers");
                if omit_mode && options.mode == Some(McpListMode::Status) {
                    // status-only 被拒时省略 mode 会退回默认 connect，重新执行 replace 并
                    // 可能改变连接集合；用稳定错误让调用方停止轮询。
                    return Err(ControlError::McpStatusModeUnsupported);
                }
                if !omit_mode && !omit_servers {
                    return Err(ControlError::Client(error));
                }
                let retry = ListMcpServersOptions {
                    mcp_servers: if omit_servers {
                        None
                    } else {
                        options.mcp_servers
                    },
                    mode: if omit_mode { None } else { options.mode },
                };
                request_mcp_list(&process, target, &retry).await
            }
            Err(error) => Err(error),
        }
    }
}

async fn request_mcp_list(
    process: &crate::AgentProcess,
    target: &AgentWorkspaceTarget,
    options: &ListMcpServersOptions,
) -> Result<McpListResult, ControlError> {
    let params = McpListParams {
        workspace: session_workspace_ref(target),
        mcp_servers: options.mcp_servers.clone(),
        mode: options.mode,
    };
    let params = serde_json::to_value(&params).expect("mcp/list params 序列化不会失败");
    let response = process
        .client()
        .request(MCP_LIST_METHOD, Some(params))
        .await?;
    serde_json::from_value(response)
        .map_err(|error| ControlError::InvalidResult(format!("invalid mcp/list result: {error}")))
}

/// TS `parseInvalidParamsIssues`：只认 `-32602` + `Invalid params…` + `data.message`
/// 中的 JSON issue 数组；其它错误不参与兼容重试。
fn parse_invalid_params_issues(error: &ClientError) -> Vec<Value> {
    let ClientError::Server {
        code,
        message,
        data,
    } = error
    else {
        return Vec::new();
    };
    if *code != -32602
        || !(message == "Invalid params" || message.starts_with("Invalid params — "))
    {
        return Vec::new();
    }
    let Some(serialized) = data
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|data| data.get("message"))
        .and_then(Value::as_str)
    else {
        return Vec::new();
    };
    serde_json::from_str::<Value>(serialized)
        .ok()
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
}

fn unrecognized_top_level_keys(error: &ClientError) -> Vec<String> {
    let mut keys = Vec::new();
    for issue in parse_invalid_params_issues(error) {
        let Some(issue) = issue.as_object() else {
            continue;
        };
        if issue.get("code").and_then(Value::as_str) != Some("unrecognized_keys") {
            continue;
        }
        if issue
            .get("path")
            .and_then(Value::as_array)
            .is_some_and(|path| !path.is_empty())
        {
            continue;
        }
        if let Some(values) = issue.get("keys").and_then(Value::as_array) {
            for value in values {
                if let Some(key) = value.as_str() {
                    keys.push(key.to_string());
                }
            }
        }
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn error(data_message: &str) -> ClientError {
        ClientError::Server {
            code: -32602,
            message: "Invalid params".to_string(),
            data: Some(json!({ "message": data_message })),
        }
    }

    #[test]
    fn parses_only_top_level_unrecognized_keys() {
        let nested = error(r#"[{"code":"unrecognized_keys","keys":["mode"],"path":["config"]}]"#);
        assert!(unrecognized_top_level_keys(&nested).is_empty());

        let top = error(r#"[{"code":"unrecognized_keys","keys":["mode","mcpServers"]}]"#);
        assert_eq!(
            unrecognized_top_level_keys(&top),
            vec!["mode".to_string(), "mcpServers".to_string()]
        );
    }

    #[test]
    fn ignores_non_matching_errors() {
        let wrong_code = ClientError::Server {
            code: -32601,
            message: "Invalid params".to_string(),
            data: Some(json!({ "message": "[]" })),
        };
        assert!(unrecognized_top_level_keys(&wrong_code).is_empty());

        let wrong_message = ClientError::Server {
            code: -32602,
            message: "Something else".to_string(),
            data: Some(json!({ "message": "[]" })),
        };
        assert!(unrecognized_top_level_keys(&wrong_message).is_empty());
    }
}
