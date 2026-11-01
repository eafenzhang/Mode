//! 远端判重（`listRemoteUserPluginStatuses`）。

use std::path::Path;

use plugin_discovery::js;

use crate::candidates::{
    collect_configured_inline_plugin_path_by_id, normalize_plugin_id, normalize_plugin_id_key,
};
use crate::paths::{resolve_within, user_mode_plugin_root};
use crate::types::{PluginSyncRemoteStatus, PluginSyncRemoteStatusReason};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteStatusQuery {
    pub plugin_id: String,
    pub directory_name: String,
}

pub fn list_remote_user_plugin_statuses(
    home: &Path,
    plugins: &[RemoteStatusQuery],
) -> Result<Vec<PluginSyncRemoteStatus>, String> {
    let target_root = user_mode_plugin_root(home);
    let configured = collect_configured_inline_plugin_path_by_id(home)?;
    let mut statuses = Vec::with_capacity(plugins.len());
    for plugin in plugins {
        let directory_name = crate::paths::normalize_relative_path(&plugin.directory_name)?;
        let plugin_id = normalize_plugin_id(&plugin.plugin_id)?;
        let target_path = resolve_within(&target_root, &directory_name)?;
        if js::exists(&target_path) {
            statuses.push(PluginSyncRemoteStatus {
                plugin_id,
                directory_name,
                exists: true,
                path: Some(target_path.to_string_lossy().into_owned()),
                reason: Some(PluginSyncRemoteStatusReason::TargetExists),
            });
            continue;
        }
        let key = normalize_plugin_id_key(&plugin_id)?;
        if let Some((_, existing_path)) = configured.iter().find(|(existing, _)| existing == &key) {
            statuses.push(PluginSyncRemoteStatus {
                plugin_id,
                directory_name,
                exists: true,
                path: Some(existing_path.clone()),
                reason: Some(PluginSyncRemoteStatusReason::SamePluginId),
            });
            continue;
        }
        statuses.push(PluginSyncRemoteStatus {
            plugin_id,
            directory_name,
            exists: false,
            path: None,
            reason: None,
        });
    }
    Ok(statuses)
}
