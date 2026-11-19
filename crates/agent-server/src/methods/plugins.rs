//! Plugins 会话面（C4 T2）：读面、启停、市场与安装写侧。
//!
//! 观察基线：`docs/specs/agent-c4-contract.md` §3。所有方法共享 `PluginService`
//! 的同一份配置/缓存事实源；`plugins/operationProgress` 不产生（T0 各窗口均为 0 条）。

use agent_protocol::plugin::{
    PluginReferenceAuthority, PluginReferenceCatalogParams, PluginsCancelOperationParams,
    PluginsCancelOperationResult, PluginsDescribeParams, PluginsInstallParams, PluginsListParams,
    PluginsMarketplaceAddParams, PluginsMarketplaceRemoveParams, PluginsMarketplaceUpdateParams,
    PluginsOverviewParams, PluginsResolveSuggestedReferenceParams, PluginsSetEnabledParams,
    PluginsUninstallParams, PluginsUpdateParams,
};
use serde_json::Value;

use crate::dispatch::AgentServer;
use crate::plugins::PluginService;
use crate::server::ServerError;

use super::{parse_params, store_error};

fn service(server: &AgentServer) -> Result<PluginService, ServerError> {
    let data_root = server
        .data_root()
        .ok_or_else(|| ServerError::internal("数据根不可用：插件配置无事实源"))?;
    Ok(PluginService::from_data_root(&data_root))
}

fn to_value<T: serde::Serialize>(value: T) -> Result<Value, ServerError> {
    serde_json::to_value(value).map_err(|error| ServerError::internal(error.to_string()))
}

fn session_authority(
    server: &AgentServer,
    session_id: Option<&String>,
) -> Result<PluginReferenceAuthority, ServerError> {
    match session_id {
        Some(session_id) => {
            let store = server.store()?;
            let exists = store
                .get_session(session_id)
                .map_err(store_error)?
                .is_some();
            if !exists {
                return Err(ServerError::rpc(
                    -32004,
                    format!("Session is not active: {session_id}"),
                    None,
                ));
            }
            Ok(PluginReferenceAuthority::Session)
        }
        None => Ok(PluginReferenceAuthority::Workspace),
    }
}

pub(crate) fn handle_list(server: &AgentServer, params: Value) -> Result<Value, ServerError> {
    let _params: PluginsListParams = parse_params(crate::methods::PLUGINS_LIST_METHOD, params)?;
    to_value(service(server)?.list())
}

pub(crate) fn handle_describe(server: &AgentServer, params: Value) -> Result<Value, ServerError> {
    let params: PluginsDescribeParams =
        parse_params(crate::methods::PLUGINS_DESCRIBE_METHOD, params)?;
    to_value(service(server)?.describe(&params.marketplace, &params.plugin_name))
}

pub(crate) fn handle_overview(server: &AgentServer, params: Value) -> Result<Value, ServerError> {
    let _params: PluginsOverviewParams =
        parse_params(crate::methods::PLUGINS_OVERVIEW_METHOD, params)?;
    to_value(service(server)?.overview())
}

pub(crate) fn handle_set_enabled(
    server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: PluginsSetEnabledParams =
        parse_params(crate::methods::PLUGINS_SET_ENABLED_METHOD, params)?;
    service(server)?
        .set_enabled(&params.plugin_id, params.enabled)
        .map_err(ServerError::invalid_params)
        .and_then(to_value)
}

pub(crate) fn handle_reference_catalog(
    server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: PluginReferenceCatalogParams =
        parse_params(crate::methods::PLUGINS_REFERENCE_CATALOG_METHOD, params)?;
    let authority = session_authority(server, params.session_id.as_ref())?;
    to_value(service(server)?.reference_catalog(authority))
}

pub(crate) fn handle_reference_catalog_with_category(
    server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: PluginReferenceCatalogParams = parse_params(
        crate::methods::PLUGINS_REFERENCE_CATALOG_WITH_CATEGORY_METHOD,
        params,
    )?;
    let authority = session_authority(server, params.session_id.as_ref())?;
    to_value(service(server)?.reference_catalog(authority))
}

pub(crate) fn handle_resolve_suggested_reference(
    server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: PluginsResolveSuggestedReferenceParams = parse_params(
        crate::methods::PLUGINS_RESOLVE_SUGGESTED_REFERENCE_METHOD,
        params,
    )?;
    to_value(service(server)?.resolve_suggested_reference(&params.stable_id))
}

pub(crate) fn handle_marketplace_add(
    server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: PluginsMarketplaceAddParams =
        parse_params(crate::methods::PLUGINS_MARKETPLACE_ADD_METHOD, params)?;
    service(server)?
        .marketplace_add(&params.source)
        .map_err(ServerError::invalid_params)
        .and_then(to_value)
}

pub(crate) fn handle_marketplace_remove(
    server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: PluginsMarketplaceRemoveParams =
        parse_params(crate::methods::PLUGINS_MARKETPLACE_REMOVE_METHOD, params)?;
    service(server)?
        .marketplace_remove(&params.marketplace)
        .map_err(ServerError::invalid_params)
        .and_then(to_value)
}

pub(crate) fn handle_marketplace_update(
    server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: PluginsMarketplaceUpdateParams =
        parse_params(crate::methods::PLUGINS_MARKETPLACE_UPDATE_METHOD, params)?;
    service(server)?
        .marketplace_update(params.marketplace.as_deref())
        .map_err(ServerError::invalid_params)
        .and_then(to_value)
}

pub(crate) fn handle_install(server: &AgentServer, params: Value) -> Result<Value, ServerError> {
    let params: PluginsInstallParams =
        parse_params(crate::methods::PLUGINS_INSTALL_METHOD, params)?;
    service(server)?
        .install(&params.plugin_name, &params.marketplace, params.scope)
        .map_err(ServerError::invalid_params)
        .and_then(to_value)
}

pub(crate) fn handle_uninstall(server: &AgentServer, params: Value) -> Result<Value, ServerError> {
    let params: PluginsUninstallParams =
        parse_params(crate::methods::PLUGINS_UNINSTALL_METHOD, params)?;
    service(server)?
        .uninstall(
            params.plugin_id.as_deref(),
            params.plugin_name.as_deref(),
            params.marketplace.as_deref(),
            params.remove_cache,
        )
        .map_err(ServerError::invalid_params)
        .and_then(to_value)
}

pub(crate) fn handle_update(server: &AgentServer, params: Value) -> Result<Value, ServerError> {
    let params: PluginsUpdateParams = parse_params(crate::methods::PLUGINS_UPDATE_METHOD, params)?;
    service(server)?
        .update(&params)
        .map_err(ServerError::invalid_params)
        .and_then(to_value)
}

/// `plugins/cancelOperation`：当前实现没有异步安装/刷新任务，显式 `cancelled:false`。
pub(crate) fn handle_cancel_operation(
    server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: PluginsCancelOperationParams =
        parse_params(crate::methods::PLUGINS_CANCEL_OPERATION_METHOD, params)?;
    let _ = service(server)?;
    to_value(PluginsCancelOperationResult {
        operation_id: params.operation_id,
        cancelled: false,
    })
}
