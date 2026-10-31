//! `skill-sync` 频道适配器：5 个方法。
//!
//! 导出的 `archive` 是嵌套 `Uint8Array`：RPC 线上以
//! `{"__mode_rpc_nested_uint8array_v1":true,"base64":…}` 标记承载，
//! 适配器在返回时构造标记、导入时还原为字节（`rpc_wire::nested_bytes`）。

use std::sync::Arc;

use rpc_host::{BoxFuture, CancellationToken, ChannelError, ServerChannel};
use rpc_wire::nested_bytes;
use rpc_wire::Value;
use serde::Deserialize;
use serde_json::json;
use skill_sync_service::{SkillSyncService, SkillSyncServiceOptions};

use crate::params::first_arg_object;

pub const LIST_LOCAL_USER_SKILL_CANDIDATES: &str = "listLocalUserSkillCandidates";
pub const LIST_REMOTE_USER_SKILL_STATUSES: &str = "listRemoteUserSkillStatuses";
pub const EXPORT_SKILLS_ARCHIVE: &str = "exportSkillsArchive";
pub const CHECK_REMOTE_USER_SKILL_WRITE_ACCESS: &str = "checkRemoteUserSkillWriteAccess";
pub const IMPORT_SKILLS_ARCHIVE: &str = "importSkillsArchive";

pub struct SkillSyncChannel {
    service: Arc<SkillSyncService>,
}

impl SkillSyncChannel {
    pub fn new(service: Arc<SkillSyncService>) -> Self {
        Self { service }
    }
}

impl ServerChannel for SkillSyncChannel {
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
                    ChannelError::error(format!("skill-sync channel task failed: {error}"))
                })?
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkillHint {
    directory_name: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteStatusArgs {
    directory_names: Vec<String>,
    #[serde(default)]
    skills: Option<Vec<SkillHint>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportArgs {
    skill_ids: Vec<String>,
}

fn run(service: &SkillSyncService, command: &str, arg: &Value) -> Result<Value, ChannelError> {
    match command {
        LIST_LOCAL_USER_SKILL_CANDIDATES => Ok(json_value(
            &serde_json::to_value(service.list_local_user_skill_candidates())
                .map_err(|error| ChannelError::error(error.to_string()))?,
        )),
        LIST_REMOTE_USER_SKILL_STATUSES => {
            let params = parse_object(arg)?;
            let args: RemoteStatusArgs = serde_json::from_value(serde_json::Value::Object(params))
                .map_err(|error| ChannelError::error(format!("invalid arguments: {error}")))?;
            let hints: Vec<(String, String)> = args
                .skills
                .unwrap_or_default()
                .into_iter()
                .map(|hint| (hint.directory_name, hint.name))
                .collect();
            let result = service
                .list_remote_user_skill_statuses(&args.directory_names, &hints)
                .map_err(ChannelError::error)?;
            Ok(json_value(
                &serde_json::to_value(&result)
                    .map_err(|error| ChannelError::error(error.to_string()))?,
            ))
        }
        EXPORT_SKILLS_ARCHIVE => {
            let args: ExportArgs = parse(arg)?;
            let result = service
                .export_skills_archive(&args.skill_ids)
                .map_err(ChannelError::error)?;
            let payload = json!({
                "archive": nested_bytes::to_marker(&result.archive),
                "archiveBytes": result.archive_bytes,
                "skills": serde_json::to_value(&result.skills)
                    .map_err(|error| ChannelError::error(error.to_string()))?,
            });
            Ok(json_value(&payload))
        }
        CHECK_REMOTE_USER_SKILL_WRITE_ACCESS => {
            Ok(json_value(&service.check_remote_user_skill_write_access()))
        }
        IMPORT_SKILLS_ARCHIVE => {
            let params = parse_object(arg)?;
            let archive_value = params
                .get("archive")
                .ok_or_else(|| ChannelError::error("invalid arguments: missing field `archive`"))?;
            let archive = nested_bytes::from_marker(archive_value).ok_or_else(|| {
                ChannelError::error("invalid arguments: archive must be a Uint8Array")
            })?;
            let overwrite = params
                .get("overwrite")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let result = service
                .import_skills_archive(&archive, overwrite)
                .map_err(ChannelError::error)?;
            Ok(json_value(
                &serde_json::to_value(&result)
                    .map_err(|error| ChannelError::error(error.to_string()))?,
            ))
        }
        other => Err(ChannelError::error(format!(
            "skill-sync channel: unknown command {other}"
        ))),
    }
}

fn parse_object(arg: &Value) -> Result<serde_json::Map<String, serde_json::Value>, ChannelError> {
    first_arg_object(arg)
}

fn parse<T: serde::de::DeserializeOwned>(arg: &Value) -> Result<T, ChannelError> {
    let params = parse_object(arg)?;
    serde_json::from_value(serde_json::Value::Object(params))
        .map_err(|error| ChannelError::error(format!("invalid arguments: {error}")))
}

fn json_value(value: &serde_json::Value) -> Value {
    match serde_json::to_string(value) {
        Ok(text) => Value::Json(text),
        Err(error) => Value::Json(format!("{{\"serializeError\":\"{error}\"}}")),
    }
}

/// 供测试构造服务。
pub fn service_for_home(home: std::path::PathBuf) -> Arc<SkillSyncService> {
    Arc::new(SkillSyncService::new(SkillSyncServiceOptions {
        home_dir: Some(home),
        max_archive_bytes: None,
    }))
}
