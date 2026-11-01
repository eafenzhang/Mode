//! 插件只读面 facade：`plugins/list` 与 `plugins/describe`。
//!
//! 行为真源：`packages/services/src/mode-agent/modeAgentService.ts` 的 `listPlugins` /
//! `describePlugin`。
//!
//! 差异：TS 使用独立的 plugin 管理进程（合成 workspace）与更长超时；本 facade 复用
//! workspace runtime（与 mcp/list 同一取舍），超时使用协议默认值。

use agent_protocol::plugin::{
    PluginScope, PluginsConfigureParams, PluginsConfigureResult, PluginsDescribeParams,
    PluginsDescribeResult, PluginsListParams, PluginsListResult, PluginsOverviewParams,
    PluginsOverviewResult, PluginsResetConfigParams, PluginsRestoreBuiltinParams,
    PluginsRestoreBuiltinResult, PluginsSetEnabledParams, PluginsSetEnabledResult,
    PluginsValidateParams, PluginsValidateResult, PLUGINS_CONFIGURE_METHOD,
    PLUGINS_DESCRIBE_METHOD, PLUGINS_LIST_METHOD, PLUGINS_OVERVIEW_METHOD,
    PLUGINS_RESET_CONFIG_METHOD, PLUGINS_RESTORE_BUILTIN_METHOD, PLUGINS_SET_ENABLED_METHOD,
    PLUGINS_VALIDATE_METHOD,
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
        let process = self
            .client_for(target, RuntimePolicy::StartIfNeeded)
            .await?;
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
        let process = self
            .client_for(target, RuntimePolicy::StartIfNeeded)
            .await?;
        let params = PluginsDescribeParams {
            workspace: session_workspace_ref(target),
            marketplace: marketplace.to_string(),
            plugin_name: plugin_name.to_string(),
        };
        let params = serde_json::to_value(&params).expect("plugins/describe params 序列化不会失败");
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
        let process = self
            .client_for(target, RuntimePolicy::StartIfNeeded)
            .await?;
        let params = PluginsOverviewParams {
            workspace: session_workspace_ref(target),
            config_scope,
        };
        let params = serde_json::to_value(&params).expect("plugins/overview params 序列化不会失败");
        let response = process
            .client()
            .request(PLUGINS_OVERVIEW_METHOD, Some(params))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!("invalid plugins/overview result: {error}"))
        })
    }

    /// `plugins/setEnabled`：启用/停用插件，返回更新后的插件信息。
    pub async fn set_plugin_enabled(
        &self,
        target: &AgentWorkspaceTarget,
        plugin_id: &str,
        enabled: bool,
        operation_id: Option<&str>,
        scope: Option<PluginScope>,
    ) -> Result<PluginsSetEnabledResult, ControlError> {
        let params = PluginsSetEnabledParams {
            workspace: session_workspace_ref(target),
            plugin_id: plugin_id.to_string(),
            enabled,
            operation_id: operation_id.map(str::to_string),
            scope,
        };
        request_plugin_policy(
            self,
            target,
            PLUGINS_SET_ENABLED_METHOD,
            &params,
            "plugins/setEnabled",
        )
        .await
    }

    /// `plugins/restoreBuiltin`：把被覆盖的内置插件恢复到包内版本。
    pub async fn restore_builtin_plugin(
        &self,
        target: &AgentWorkspaceTarget,
        plugin_id: &str,
    ) -> Result<PluginsRestoreBuiltinResult, ControlError> {
        let params = PluginsRestoreBuiltinParams {
            workspace: session_workspace_ref(target),
            plugin_id: plugin_id.to_string(),
        };
        request_plugin_policy(
            self,
            target,
            PLUGINS_RESTORE_BUILTIN_METHOD,
            &params,
            "plugins/restoreBuiltin",
        )
        .await
    }

    /// `plugins/configure`：写入用户配置；`dryRun` 只校验不落盘。
    pub async fn configure_plugin(
        &self,
        target: &AgentWorkspaceTarget,
        plugin_id: &str,
        options: std::collections::BTreeMap<String, serde_json::Value>,
        clear_option_keys: Option<Vec<String>>,
        scope: Option<PluginScope>,
        dry_run: Option<bool>,
    ) -> Result<PluginsConfigureResult, ControlError> {
        let params = PluginsConfigureParams {
            workspace: session_workspace_ref(target),
            plugin_id: plugin_id.to_string(),
            options,
            clear_option_keys,
            scope,
            dry_run,
        };
        request_plugin_policy(
            self,
            target,
            PLUGINS_CONFIGURE_METHOD,
            &params,
            "plugins/configure",
        )
        .await
    }

    /// `plugins/resetConfig`：清空用户配置，结果与 configure 同形。
    pub async fn reset_plugin_config(
        &self,
        target: &AgentWorkspaceTarget,
        plugin_id: &str,
        scope: Option<PluginScope>,
    ) -> Result<PluginsConfigureResult, ControlError> {
        let params = PluginsResetConfigParams {
            workspace: session_workspace_ref(target),
            plugin_id: plugin_id.to_string(),
            scope,
        };
        request_plugin_policy(
            self,
            target,
            PLUGINS_RESET_CONFIG_METHOD,
            &params,
            "plugins/resetConfig",
        )
        .await
    }

    /// `plugins/validate`：按名称/市场/源之一校验包，返回兼容性分组。
    pub async fn validate_plugin(
        &self,
        target: &AgentWorkspaceTarget,
        plugin_name: Option<&str>,
        marketplace: Option<&str>,
        source: Option<&str>,
    ) -> Result<PluginsValidateResult, ControlError> {
        let params = PluginsValidateParams {
            workspace: session_workspace_ref(target),
            plugin_name: plugin_name.map(str::to_string),
            marketplace: marketplace.map(str::to_string),
            source: source.map(str::to_string),
        };
        request_plugin_policy(
            self,
            target,
            PLUGINS_VALIDATE_METHOD,
            &params,
            "plugins/validate",
        )
        .await
    }
}

async fn request_plugin_policy<TParams, TResult>(
    client: &AgentControlClient,
    target: &AgentWorkspaceTarget,
    method: &'static str,
    params: &TParams,
    label: &'static str,
) -> Result<TResult, ControlError>
where
    TParams: serde::Serialize,
    TResult: serde::de::DeserializeOwned,
{
    let process = client
        .client_for(target, RuntimePolicy::StartIfNeeded)
        .await?;
    let value = serde_json::to_value(params).map_err(|error| {
        ControlError::InvalidParams(format!("{label} params serialization failed: {error}"))
    })?;
    let response = process.client().request(method, Some(value)).await?;
    serde_json::from_value::<TResult>(response)
        .map_err(|error| ControlError::InvalidResult(format!("invalid {label} result: {error}")))
}
