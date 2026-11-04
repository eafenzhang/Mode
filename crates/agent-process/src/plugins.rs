//! 插件只读面 facade：`plugins/list` 与 `plugins/describe`。
//!
//! 行为真源：`packages/services/src/mode-agent/modeAgentService.ts` 的 `listPlugins` /
//! `describePlugin`。
//!
//! 差异：TS 使用独立的 plugin 管理进程（合成 workspace）与更长超时；本 facade 复用
//! workspace runtime（与 mcp/list 同一取舍），超时使用协议默认值。

use agent_protocol::plugin::{
    PluginDeliveryKind, PluginOperationProgressNotification, PluginReferenceCatalogParams,
    PluginScope, PluginsCancelOperationParams,
    PluginsCancelOperationResult, PluginsConfigureParams, PluginsConfigureResult,
    PluginsDescribeParams, PluginsDescribeResult, PluginsInstallParams, PluginsInstallResult,
    PluginsListParams, PluginsListResult, PluginsMarketplaceAddParams,
    PluginsMarketplaceMutationResult, PluginsMarketplaceRemoveParams,
    PluginsMarketplaceUpdateParams, PluginsOverviewParams, PluginsOverviewResult,
    PluginsReferenceCatalogResult, PluginsResetConfigParams,
    PluginsResolveSuggestedReferenceParams, PluginsResolveSuggestedReferenceResult,
    PluginsRestoreBuiltinParams, PluginsRestoreBuiltinResult, PluginsSetEnabledParams,
    PluginsSetEnabledResult, PluginsUninstallParams, PluginsUninstallResult, PluginsUpdateParams,
    PluginsValidateParams, PluginsValidateResult, PLUGINS_CANCEL_OPERATION_METHOD,
    PLUGINS_CONFIGURE_METHOD, PLUGINS_DESCRIBE_METHOD, PLUGINS_INSTALL_METHOD, PLUGINS_LIST_METHOD,
    PLUGINS_MARKETPLACE_ADD_METHOD, PLUGINS_MARKETPLACE_REMOVE_METHOD,
    PLUGINS_MARKETPLACE_UPDATE_METHOD, PLUGINS_OVERVIEW_METHOD, PLUGINS_REFERENCE_CATALOG_METHOD,
    PLUGINS_REFERENCE_CATALOG_WITH_CATEGORY_METHOD, PLUGINS_RESET_CONFIG_METHOD,
    PLUGINS_RESOLVE_SUGGESTED_REFERENCE_METHOD, PLUGINS_RESTORE_BUILTIN_METHOD,
    PLUGINS_SET_ENABLED_METHOD, PLUGINS_UNINSTALL_METHOD, PLUGINS_UPDATE_METHOD,
    PLUGINS_VALIDATE_METHOD, PLUGIN_OPERATION_PROGRESS_NOTIFICATION,
};
use mode_protocol_client::ClientError;
use tokio::sync::mpsc;

use crate::control::{AgentControlClient, AgentWorkspaceTarget, ControlError, RuntimePolicy};
use crate::session_read::session_workspace_ref;

/// 插件操作进度通知流；receiver 关闭即退订。
pub struct PluginOperationProgressSubscription {
    pub receiver: mpsc::Receiver<PluginOperationProgressNotification>,
}

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

    /// 对话引用 catalog：优先 `referenceCatalogWithCategory`，旧 Agent `-32601`
    /// 时回退到 `referenceCatalog`（两者 params/result 同形）。
    pub async fn plugin_reference_catalog(
        &self,
        target: &AgentWorkspaceTarget,
        session_id: Option<&str>,
    ) -> Result<PluginsReferenceCatalogResult, ControlError> {
        let params = PluginReferenceCatalogParams {
            workspace: session_workspace_ref(target),
            session_id: session_id.map(str::to_string),
        };
        let with_category: Result<PluginsReferenceCatalogResult, ControlError> =
            request_plugin_policy(
                self,
                target,
                PLUGINS_REFERENCE_CATALOG_WITH_CATEGORY_METHOD,
                &params,
                "plugins/referenceCatalogWithCategory",
            )
            .await;
        match with_category {
            Err(ControlError::Client(ClientError::Server { code: -32601, .. })) => {
                request_plugin_policy(
                    self,
                    target,
                    PLUGINS_REFERENCE_CATALOG_METHOD,
                    &params,
                    "plugins/referenceCatalog",
                )
                .await
            }
            other => other,
        }
    }

    /// `plugins/resolveSuggestedReference`：可信安装身份解析；结果再做
    /// actionable 状态的 `superRefine`。
    pub async fn resolve_suggested_plugin_reference(
        &self,
        target: &AgentWorkspaceTarget,
        stable_id: &str,
        operation_id: &str,
        client_mode: PluginDeliveryKind,
        delivery_kind: PluginDeliveryKind,
    ) -> Result<PluginsResolveSuggestedReferenceResult, ControlError> {
        let params = PluginsResolveSuggestedReferenceParams {
            workspace: session_workspace_ref(target),
            stable_id: stable_id.to_string(),
            operation_id: operation_id.to_string(),
            client_mode,
            delivery_kind,
        };
        let result: PluginsResolveSuggestedReferenceResult = request_plugin_policy(
            self,
            target,
            PLUGINS_RESOLVE_SUGGESTED_REFERENCE_METHOD,
            &params,
            "plugins/resolveSuggestedReference",
        )
        .await?;
        result
            .validate()
            .map_err(|message| ControlError::InvalidResult(message.to_string()))?;
        Ok(result)
    }

    /// `plugins/marketplace/add`。
    pub async fn add_plugin_marketplace(
        &self,
        target: &AgentWorkspaceTarget,
        source: &str,
        dry_run: Option<bool>,
        operation_id: Option<&str>,
    ) -> Result<PluginsMarketplaceMutationResult, ControlError> {
        let params = PluginsMarketplaceAddParams {
            workspace: session_workspace_ref(target),
            source: source.to_string(),
            dry_run,
            operation_id: operation_id.map(str::to_string),
        };
        request_plugin_policy(
            self,
            target,
            PLUGINS_MARKETPLACE_ADD_METHOD,
            &params,
            "plugins/marketplace/add",
        )
        .await
    }

    /// `plugins/marketplace/remove`。
    pub async fn remove_plugin_marketplace(
        &self,
        target: &AgentWorkspaceTarget,
        marketplace: &str,
    ) -> Result<PluginsMarketplaceMutationResult, ControlError> {
        let params = PluginsMarketplaceRemoveParams {
            workspace: session_workspace_ref(target),
            marketplace: marketplace.to_string(),
        };
        request_plugin_policy(
            self,
            target,
            PLUGINS_MARKETPLACE_REMOVE_METHOD,
            &params,
            "plugins/marketplace/remove",
        )
        .await
    }

    /// `plugins/marketplace/update`：`marketplace` 缺省表示刷新全部市场。
    pub async fn update_plugin_marketplace(
        &self,
        target: &AgentWorkspaceTarget,
        marketplace: Option<&str>,
        operation_id: Option<&str>,
    ) -> Result<PluginsMarketplaceMutationResult, ControlError> {
        let params = PluginsMarketplaceUpdateParams {
            workspace: session_workspace_ref(target),
            marketplace: marketplace.map(str::to_string),
            operation_id: operation_id.map(str::to_string),
        };
        request_plugin_policy(
            self,
            target,
            PLUGINS_MARKETPLACE_UPDATE_METHOD,
            &params,
            "plugins/marketplace/update",
        )
        .await
    }

    /// `plugins/install`。
    pub async fn install_plugin(
        &self,
        target: &AgentWorkspaceTarget,
        plugin_name: &str,
        marketplace: &str,
        scope: Option<PluginScope>,
        dry_run: Option<bool>,
        operation_id: Option<&str>,
    ) -> Result<PluginsInstallResult, ControlError> {
        let params = PluginsInstallParams {
            workspace: session_workspace_ref(target),
            plugin_name: plugin_name.to_string(),
            marketplace: marketplace.to_string(),
            scope,
            dry_run,
            operation_id: operation_id.map(str::to_string),
        };
        request_plugin_policy(
            self,
            target,
            PLUGINS_INSTALL_METHOD,
            &params,
            "plugins/install",
        )
        .await
    }

    /// `plugins/operationProgress` 通知订阅（按 operationId 过滤）。
    ///
    /// 先订阅通知广播再返回；receiver 被丢弃时转发任务自行退出。
    pub async fn subscribe_plugin_operation_progress(
        &self,
        target: &AgentWorkspaceTarget,
        operation_id: &str,
    ) -> Result<PluginOperationProgressSubscription, ControlError> {
        let process = self
            .client_for(target, RuntimePolicy::StartIfNeeded)
            .await?;
        let mut notifications = process.client().subscribe_notifications();
        let operation_id = operation_id.to_owned();
        let (sender, receiver) = mpsc::channel(32);
        tokio::spawn(async move {
            loop {
                match notifications.recv().await {
                    Ok(value) => {
                        if value.get("method").and_then(serde_json::Value::as_str)
                            != Some(PLUGIN_OPERATION_PROGRESS_NOTIFICATION)
                        {
                            continue;
                        }
                        let Some(params) = value.get("params") else {
                            continue;
                        };
                        let Ok(notification) =
                            serde_json::from_value::<PluginOperationProgressNotification>(
                                params.clone(),
                            )
                        else {
                            continue;
                        };
                        if notification.operation_id != operation_id {
                            continue;
                        }
                        if sender.send(notification).await.is_err() {
                            return;
                        }
                    }
                    Err(_) => return,
                }
            }
        });
        Ok(PluginOperationProgressSubscription { receiver })
    }

    /// `plugins/cancelOperation`：只带 operationId，不带 workspace。
    pub async fn cancel_plugin_operation(
        &self,
        target: &AgentWorkspaceTarget,
        operation_id: &str,
    ) -> Result<PluginsCancelOperationResult, ControlError> {
        let params = PluginsCancelOperationParams {
            operation_id: operation_id.to_string(),
        };
        request_plugin_policy(
            self,
            target,
            PLUGINS_CANCEL_OPERATION_METHOD,
            &params,
            "plugins/cancelOperation",
        )
        .await
    }

    /// `plugins/uninstall`：pluginId / pluginName+marketplace 二选一由调用方保证。
    pub async fn uninstall_plugin(
        &self,
        target: &AgentWorkspaceTarget,
        plugin_id: Option<&str>,
        plugin_name: Option<&str>,
        marketplace: Option<&str>,
        remove_cache: Option<bool>,
    ) -> Result<PluginsUninstallResult, ControlError> {
        let params = PluginsUninstallParams {
            workspace: session_workspace_ref(target),
            plugin_id: plugin_id.map(str::to_string),
            plugin_name: plugin_name.map(str::to_string),
            marketplace: marketplace.map(str::to_string),
            remove_cache,
        };
        request_plugin_policy(
            self,
            target,
            PLUGINS_UNINSTALL_METHOD,
            &params,
            "plugins/uninstall",
        )
        .await
    }

    /// `plugins/update`：结果与 install 同形。
    pub async fn update_plugin(
        &self,
        target: &AgentWorkspaceTarget,
        plugin_id: Option<&str>,
        marketplace: Option<&str>,
    ) -> Result<PluginsInstallResult, ControlError> {
        let params = PluginsUpdateParams {
            workspace: session_workspace_ref(target),
            plugin_id: plugin_id.map(str::to_string),
            marketplace: marketplace.map(str::to_string),
        };
        request_plugin_policy(
            self,
            target,
            PLUGINS_UPDATE_METHOD,
            &params,
            "plugins/update",
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
