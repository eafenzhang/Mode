//! `hooks` 频道适配器：`loadHooks` / `saveHooks` / `grantWorkspaceHookTrust`。
//!
//! 真源：`packages/services/src/hooks/hooks.ts` 与 `hooksService.ts`。
//! 读侧合并、保存拆分与 trust store 都在 `workspace-hooks-service`；本层只做参数形状、
//! 身份传递与 grant 转发（必须走 Agent authority，不能直接写 trust store）。
//!
//! 参数是 `ProxyChannel` 的位置参数数组（第一个元素为对象）；同步文件 IO 走
//! `spawn_blocking`，不阻塞 RPC 执行器。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use agent_process::{AgentControlClient, AgentWorkspaceTarget};
use rpc_host::{BoxFuture, CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;
use serde::Deserialize;
use serde_json::json;
use workspace_hooks_service::{load_hooks_with_identity, save_hooks};

use crate::params::first_arg_object;

pub const HOOKS_CHANNEL: &str = "hooks";
pub const LOAD_HOOKS: &str = "loadHooks";
pub const SAVE_HOOKS: &str = "saveHooks";
pub const GRANT_WORKSPACE_HOOK_TRUST: &str = "grantWorkspaceHookTrust";

pub struct HooksChannel {
    control: Arc<AgentControlClient>,
    home: PathBuf,
}

impl HooksChannel {
    pub fn new(control: Arc<AgentControlClient>) -> Self {
        Self::with_home_dir(control, process_home_dir())
    }

    /// 测试/嵌入宿主可注入 home，避免读到真实用户级 hook 配置。
    pub fn with_home_dir(control: Arc<AgentControlClient>, home: PathBuf) -> Self {
        Self { control, home }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoadParams {
    workspace_path: String,
    #[serde(default)]
    workspace_identity: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveParams {
    workspace_path: String,
    hooks: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GrantParams {
    workspace_path: String,
    #[serde(default)]
    workspace_identity: Option<String>,
    bundle_digest: String,
    hook_declaration_digest: String,
}

fn parse_params<T: serde::de::DeserializeOwned>(
    command: &str,
    arg: Value,
) -> Result<T, ChannelError> {
    let object = first_arg_object(&arg)?;
    serde_json::from_value(serde_json::Value::Object(object))
        .map_err(|error| ChannelError::error(format!("invalid {command} request: {error}")))
}

fn json_text(value: serde_json::Value) -> Value {
    Value::Json(value.to_string())
}

fn process_home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// TS `resolve(workspacePath)` 的等价：相对路径按当前目录展开，已是绝对路径则原样。
fn absolute_workspace_path(value: &str) -> PathBuf {
    std::path::absolute(Path::new(value)).unwrap_or_else(|_| PathBuf::from(value))
}

impl ServerChannel for HooksChannel {
    fn call(
        &self,
        _ctx: String,
        command: String,
        arg: Value,
        _cancel: CancellationToken,
    ) -> BoxFuture<Result<Value, ChannelError>> {
        let control = Arc::clone(&self.control);
        let home = self.home.clone();
        Box::pin(async move {
            match command.as_str() {
                LOAD_HOOKS => {
                    let params: LoadParams = parse_params("loadHooks", arg)?;
                    let discovered_at = iso8601_utc_now();
                    let load_home = home.clone();
                    let result = tokio::task::spawn_blocking(move || {
                        load_hooks_with_identity(
                            Path::new(&params.workspace_path),
                            params.workspace_identity.as_deref(),
                            &load_home,
                            &discovered_at,
                        )
                    })
                    .await
                    .map_err(|error| {
                        ChannelError::error(format!("hooks load task failed: {error}"))
                    })?;

                    let mut response = serde_json::Map::new();
                    response.insert("hooks".into(), json!(result.hooks));
                    response.insert("hooksEnabled".into(), json!(result.hooks_enabled));
                    if let Some(snapshot) = result.snapshot {
                        response.insert("workspaceHookSnapshot".into(), snapshot);
                    }
                    if result.trust_store_corrupt {
                        response.insert("trustStoreCorrupt".into(), json!(true));
                    }
                    Ok(json_text(serde_json::Value::Object(response)))
                }
                SAVE_HOOKS => {
                    let params: SaveParams = parse_params("saveHooks", arg)?;
                    let save_home = home.clone();
                    let workspace_path = absolute_workspace_path(&params.workspace_path);
                    tokio::task::spawn_blocking(move || {
                        save_hooks(&workspace_path, &params.hooks, &save_home)
                    })
                    .await
                    .map_err(|error| {
                        ChannelError::error(format!("hooks save task failed: {error}"))
                    })?
                    .map_err(|error| ChannelError::error(error.to_string()))?;
                    Ok(Value::Undefined)
                }
                GRANT_WORKSPACE_HOOK_TRUST => {
                    let params: GrantParams = parse_params("grantWorkspaceHookTrust", arg)?;
                    let mut target = AgentWorkspaceTarget::new(params.workspace_path);
                    if let Some(identity) = params.workspace_identity {
                        target = target.with_identity(identity);
                    }
                    let result = control
                        .grant_workspace_hook_trust(
                            &target,
                            &params.bundle_digest,
                            &params.hook_declaration_digest,
                        )
                        .await
                        .map_err(|error| ChannelError::error(error.to_string()))?;
                    Ok(json_text(serde_json::to_value(&result).map_err(|error| {
                        ChannelError::error(format!("serialize result failed: {error}"))
                    })?))
                }
                other => Err(ChannelError::error(format!(
                    "hooks channel: unknown command {other}"
                ))),
            }
        })
    }
}

fn iso8601_utc_now() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0);
    iso8601_utc(millis)
}

/// `new Date(ms).toISOString()`（UTC `YYYY-MM-DDTHH:mm:ss.sssZ`）。
fn iso8601_utc(ms: i64) -> String {
    fn civil_from_days(days: i64) -> (i64, u32, u32) {
        let z = days + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        (if m <= 2 { y + 1 } else { y }, m, d)
    }

    let days = ms.div_euclid(86_400_000);
    let ms_of_day = ms.rem_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    let hour = ms_of_day / 3_600_000;
    let minute = (ms_of_day / 60_000) % 60;
    let second = (ms_of_day / 1_000) % 60;
    let millis = ms_of_day % 1_000;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso8601_matches_to_iso_string() {
        assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso8601_utc(1_772_323_201_234), "2026-03-01T00:00:01.234Z");
    }
}
