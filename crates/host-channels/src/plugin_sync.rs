//! `plugin-sync` 频道适配器：7 个方法。
//!
//! `archive` 是嵌套 `Uint8Array`：返回时构造
//! `{"__mode_rpc_nested_uint8array_v1":true,"base64":…}` 标记，导入时还原为字节
//! （`rpc_wire::nested_bytes`）。行为契约见 `docs/specs/plugin-sync-service.md`。

use std::sync::Arc;

use plugin_sync_service::{PluginSyncService, PluginSyncServiceOptions, RemoteStatusQuery};
use rpc_host::{BoxFuture, CancellationToken, ChannelError, ServerChannel};
use rpc_wire::nested_bytes;
use rpc_wire::Value;
use serde::Deserialize;
use serde_json::json;

use crate::params::first_arg_object;

pub const LIST_LOCAL_USER_PLUGIN_CANDIDATES: &str = "listLocalUserPluginCandidates";
pub const LIST_REMOTE_USER_PLUGIN_STATUSES: &str = "listRemoteUserPluginStatuses";
pub const EXPORT_PLUGINS_ARCHIVE: &str = "exportPluginsArchive";
pub const EXPORT_MARKETPLACE_SOURCE_ARCHIVE: &str = "exportMarketplaceSourceArchive";
pub const IMPORT_PLUGINS_ARCHIVE: &str = "importPluginsArchive";
pub const CHECK_REMOTE_USER_PLUGIN_WRITE_ACCESS: &str = "checkRemoteUserPluginWriteAccess";
pub const IMPORT_MARKETPLACE_SOURCE_ARCHIVE: &str = "importMarketplaceSourceArchive";

pub struct PluginSyncChannel {
    service: Arc<PluginSyncService>,
}

impl PluginSyncChannel {
    pub fn new(service: Arc<PluginSyncService>) -> Self {
        Self { service }
    }
}

impl ServerChannel for PluginSyncChannel {
    fn call(
        &self,
        _ctx: String,
        command: String,
        arg: Value,
        _cancel: CancellationToken,
    ) -> BoxFuture<Result<Value, ChannelError>> {
        let service = Arc::clone(&self.service);
        Box::pin(async move {
            tokio::task::spawn_blocking(move || run(&service, &command, &arg))
                .await
                .map_err(|error| {
                    ChannelError::error(format!("plugin-sync channel task failed: {error}"))
                })?
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteStatusArgs {
    plugins: Vec<RemoteStatusQueryItem>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteStatusQueryItem {
    plugin_id: String,
    directory_name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportPluginsArgs {
    plugin_ids: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportMarketplaceArgs {
    marketplace_id: String,
    plugin_names: Vec<String>,
    source: serde_json::Value,
}

fn run(service: &PluginSyncService, command: &str, arg: &Value) -> Result<Value, ChannelError> {
    match command {
        LIST_LOCAL_USER_PLUGIN_CANDIDATES => Ok(json_value(
            &serde_json::to_value(
                service
                    .list_local_user_plugin_candidates()
                    .map_err(ChannelError::error)?,
            )
            .map_err(|error| ChannelError::error(error.to_string()))?,
        )),
        LIST_REMOTE_USER_PLUGIN_STATUSES => {
            let args: RemoteStatusArgs = parse(arg)?;
            let queries: Vec<RemoteStatusQuery> = args
                .plugins
                .into_iter()
                .map(|plugin| RemoteStatusQuery {
                    plugin_id: plugin.plugin_id,
                    directory_name: plugin.directory_name,
                })
                .collect();
            let result = service
                .list_remote_user_plugin_statuses(&queries)
                .map_err(ChannelError::error)?;
            Ok(json_value(
                &serde_json::to_value(&result)
                    .map_err(|error| ChannelError::error(error.to_string()))?,
            ))
        }
        EXPORT_PLUGINS_ARCHIVE => {
            let args: ExportPluginsArgs = parse(arg)?;
            let result = service
                .export_plugins_archive(&args.plugin_ids)
                .map_err(ChannelError::error)?;
            let payload = json!({
                "archive": nested_bytes::to_marker(&result.archive),
                "archiveBytes": result.archive_bytes,
                "plugins": serde_json::to_value(&result.plugins)
                    .map_err(|error| ChannelError::error(error.to_string()))?,
            });
            Ok(json_value(&payload))
        }
        EXPORT_MARKETPLACE_SOURCE_ARCHIVE => {
            let args: ExportMarketplaceArgs = parse(arg)?;
            let result = service
                .export_marketplace_source_archive(
                    &args.marketplace_id,
                    &args.plugin_names,
                    &args.source,
                )
                .map_err(ChannelError::error)?;
            let payload = json!({
                "archive": nested_bytes::to_marker(&result.archive),
                "archiveBytes": result.archive_bytes,
                "marketplaceId": result.marketplace_id,
                "pluginNames": result.plugin_names,
            });
            Ok(json_value(&payload))
        }
        IMPORT_PLUGINS_ARCHIVE => {
            let (archive, overwrite) = parse_archive(arg, "plugins")?;
            let result = service
                .import_plugins_archive(&archive, overwrite)
                .map_err(ChannelError::error)?;
            Ok(json_value(
                &serde_json::to_value(&result)
                    .map_err(|error| ChannelError::error(error.to_string()))?,
            ))
        }
        CHECK_REMOTE_USER_PLUGIN_WRITE_ACCESS => Ok(json_value(
            &serde_json::to_value(service.check_remote_user_plugin_write_access())
                .map_err(|error| ChannelError::error(error.to_string()))?,
        )),
        IMPORT_MARKETPLACE_SOURCE_ARCHIVE => {
            let (archive, overwrite) = parse_archive(arg, "marketplace")?;
            let result = service
                .import_marketplace_source_archive(&archive, overwrite)
                .map_err(ChannelError::error)?;
            Ok(json_value(
                &serde_json::to_value(&result)
                    .map_err(|error| ChannelError::error(error.to_string()))?,
            ))
        }
        other => Err(ChannelError::error(format!(
            "plugin-sync channel: unknown command {other}"
        ))),
    }
}

fn parse_archive(arg: &Value, label: &str) -> Result<(Vec<u8>, bool), ChannelError> {
    let params = first_arg_object(arg)?;
    let archive_value = params.get("archive").ok_or_else(|| {
        ChannelError::error(format!(
            "invalid arguments: missing field `archive` for {label}"
        ))
    })?;
    let archive = nested_bytes::from_marker(archive_value)
        .ok_or_else(|| ChannelError::error("invalid arguments: archive must be a Uint8Array"))?;
    let overwrite = params
        .get("overwrite")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    Ok((archive, overwrite))
}

fn parse<T: serde::de::DeserializeOwned>(arg: &Value) -> Result<T, ChannelError> {
    let params = first_arg_object(arg)?;
    serde_json::from_value(serde_json::Value::Object(params))
        .map_err(|error| ChannelError::error(format!("invalid arguments: {error}")))
}

fn json_value(value: &serde_json::Value) -> Value {
    match serde_json::to_string(value) {
        Ok(text) => Value::Json(text),
        Err(error) => Value::Json(format!("{{\"serializeError\":\"{error}\"}}")),
    }
}

/// 默认用户 home 的频道服务。
pub fn default_service() -> Arc<PluginSyncService> {
    Arc::new(PluginSyncService::with_default_home())
}

/// 供测试构造服务。
pub fn service_for_home(home: std::path::PathBuf) -> Arc<PluginSyncService> {
    Arc::new(PluginSyncService::new(PluginSyncServiceOptions::new(home)))
}
