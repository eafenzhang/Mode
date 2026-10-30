//! 插件只读面 facade：`plugins/list` 与 `plugins/describe`。
//!
//! 行为真源：`packages/services/src/mode-agent/modeAgentService.ts` 的 `listPlugins` /
//! `describePlugin`。
//!
//! 差异：TS 使用独立的 plugin 管理进程（合成 workspace）与更长超时；本 facade 复用
//! workspace runtime（与 mcp/list 同一取舍），超时使用协议默认值。

use agent_protocol::plugin::{
    PluginScope, PluginsDescribeParams, PluginsDescribeResult, PluginsListParams, PluginsListResult,
    PluginsOverviewParams, PluginsOverviewResult, PLUGINS_DESCRIBE_METHOD, PLUGINS_LIST_METHOD,
    PLUGINS_OVERVIEW_METHOD,
};

use crate::control::{AgentControlClient, AgentWorkspaceTarget, ControlError, RuntimePolicy};
use crate::session_read::session_workspace_ref;

impl AgentControlClient {
    /// `plugins/list`：本地插件元数据 + 诊断。
    pub async fn list_plugins(
        &self,
        target: &AgentWorkspaceTarget,
        config_scope: Option<PluginScope>,
    ) -> Result<PluginsListResult, ControlError> {
        let process = self.client_for(target, RuntimePolicy::StartIfNeeded).await?;
        let params = PluginsListParams {
            workspace: session_workspace_ref(target),
            config_scope,
        };
        let params = serde_json::to_value(&params).expect("plugins/list params 序列化不会失败");
        let response = process
            .client()
            .request(PLUGINS_LIST_METHOD, Some(params))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!("invalid plugins/list result: {error}"))
        })
    }

    /// `plugins/describe`：组件清单（agent/command/skill/hook/mcp 分组）。
    pub async fn describe_plugin(
        &self,
        target: &AgentWorkspaceTarget,
        marketplace: &str,
        plugin_name: &str,
    ) -> Result<PluginsDescribeResult, ControlError> {
        let process = self.client_for(target, RuntimePolicy::StartIfNeeded).await?;
        let params = PluginsDescribeParams {
            workspace: session_workspace_ref(target),
            marketplace: marketplace.to_string(),
            plugin_name: plugin_name.to_string(),
        };
        let params =
            serde_json::to_value(&params).expect("plugins/describe params 序列化不会失败");
        let response = process
            .client()
            .request(PLUGINS_DESCRIBE_METHOD, Some(params))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!("invalid plugins/describe result: {error}"))
        })
    }

    /// `plugins/overview`：市场 / 可安装 / 已安装 / 可恢复内置 + capability。
    pub async fn get_plugins_overview(
        &self,
        target: &AgentWorkspaceTarget,
        config_scope: Option<PluginScope>,
    ) -> Result<PluginsOverviewResult, ControlError> {
        let process = self.client_for(target, RuntimePolicy::StartIfNeeded).await?;
        let params = PluginsOverviewParams {
            workspace: session_workspace_ref(target),
            config_scope,
        };
        let params =
            serde_json::to_value(&params).expect("plugins/overview params 序列化不会失败");
        let response = process
            .client()
            .request(PLUGINS_OVERVIEW_METHOD, Some(params))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!("invalid plugins/overview result: {error}"))
        })
    }
}
