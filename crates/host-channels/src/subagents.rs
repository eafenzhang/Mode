//! `subagents` 频道适配器：8 个方法。
//!
//! 参数形状是 `ProxyChannel` 的位置参数数组（第一个元素为对象）；服务是同步文件实现，
//! 统一丢进 `spawn_blocking`，避免阻塞 RPC 执行器。
//!
//! 行为契约见 `docs/specs/subagents-service.md`。

use std::sync::Arc;

use rpc_host::{BoxFuture, CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;
use serde::Deserialize;
use serde_json::json;
use subagents_service::{
    AgentConfig, CreateAgentParams, DeleteAgentParams, ListMode, ListParams, Selection,
    SubagentsError, SubagentsService, UpdateAgentParams,
};

use crate::params::first_arg_object;

pub const LIST: &str = "list";
pub const SET_ENABLED: &str = "setEnabled";
pub const SET_BUILT_IN_MODEL_OVERRIDE: &str = "setBuiltInModelOverride";
pub const SET_PLUGIN_AGENT_MODEL_OVERRIDE: &str = "setPluginAgentModelOverride";
pub const GET_PRIMARY_USER_AGENTS_DIRECTORY: &str = "getPrimaryUserAgentsDirectory";
pub const CREATE_AGENT: &str = "createAgent";
pub const UPDATE_AGENT: &str = "updateAgent";
pub const DELETE_AGENT: &str = "deleteAgent";

pub struct SubagentsChannel {
    service: Arc<SubagentsService>,
}

impl SubagentsChannel {
    pub fn new(service: Arc<SubagentsService>) -> Self {
        Self { service }
    }
}

impl ServerChannel for SubagentsChannel {
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
                    ChannelError::error(format!("subagents channel task failed: {error}"))
                })?
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListArgs {
    workspace_path: String,
    #[serde(default)]
    workspace_identity: Option<String>,
    #[serde(default)]
    mode: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetEnabledArgs {
    agent_id: String,
    enabled: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuiltInOverrideArgs {
    agent_name: String,
    #[serde(default)]
    model_selection: Option<Selection>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginOverrideArgs {
    agent_id: String,
    #[serde(default)]
    model_selection: Option<Selection>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct CreateAgentArgs {
    config: AgentConfig,
    scope: Option<String>,
    workspace_path: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct UpdateAgentArgs {
    agent_id: String,
    config: AgentConfig,
    old_file_path: Option<String>,
    scope: Option<String>,
    workspace_path: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteAgentArgs {
    agent_id: String,
    file_path: String,
}

fn run(service: &SubagentsService, command: &str, arg: &Value) -> Result<Value, ChannelError> {
    match command {
        LIST => {
            let args: ListArgs = parse(arg)?;
            let mode = match args.mode.as_deref() {
                Some("settingsUserOnly") => Some(ListMode::SettingsUserOnly),
                Some("allRuntimeScopes") | None => None,
                Some(other) => {
                    return Err(ChannelError::error(format!(
                        "invalid arguments: unknown list mode {other}"
                    )))
                }
            };
            let result = service.list(&ListParams {
                workspace_path: args.workspace_path.into(),
                workspace_identity: args.workspace_identity,
                mode,
            });
            Ok(json_value(
                &serde_json::to_value(&result).unwrap_or(serde_json::Value::Null),
            ))
        }
        SET_ENABLED => {
            let args: SetEnabledArgs = parse(arg)?;
            service.set_enabled(&args.agent_id, args.enabled).chan()?;
            Ok(Value::Undefined)
        }
        SET_BUILT_IN_MODEL_OVERRIDE => {
            let args: BuiltInOverrideArgs = parse(arg)?;
            service
                .set_built_in_model_override(&args.agent_name, args.model_selection)
                .chan()?;
            Ok(Value::Undefined)
        }
        SET_PLUGIN_AGENT_MODEL_OVERRIDE => {
            let args: PluginOverrideArgs = parse(arg)?;
            service
                .set_plugin_agent_model_override(&args.agent_id, args.model_selection)
                .chan()?;
            Ok(Value::Undefined)
        }
        GET_PRIMARY_USER_AGENTS_DIRECTORY => {
            let path = service.get_primary_user_agents_directory().chan()?;
            Ok(json_value(&json!({ "path": path.to_string_lossy() })))
        }
        CREATE_AGENT => {
            let args: CreateAgentArgs = parse(arg)?;
            let agent = service
                .create_agent(&CreateAgentParams {
                    config: args.config,
                    scope: args.scope,
                    workspace_path: args.workspace_path,
                })
                .chan()?;
            Ok(json_value(&agent.to_json()))
        }
        UPDATE_AGENT => {
            let args: UpdateAgentArgs = parse(arg)?;
            let agent = service
                .update_agent(&UpdateAgentParams {
                    agent_id: args.agent_id,
                    config: args.config,
                    old_file_path: args.old_file_path,
                    scope: args.scope,
                    workspace_path: args.workspace_path,
                })
                .chan()?;
            Ok(json_value(&agent.to_json()))
        }
        DELETE_AGENT => {
            let args: DeleteAgentArgs = parse(arg)?;
            service
                .delete_agent(&DeleteAgentParams {
                    agent_id: args.agent_id,
                    file_path: args.file_path,
                })
                .chan()?;
            Ok(Value::Undefined)
        }
        other => Err(ChannelError::error(format!(
            "subagents channel: unknown command {other}"
        ))),
    }
}

fn parse<T: serde::de::DeserializeOwned>(arg: &Value) -> Result<T, ChannelError> {
    let params = first_arg_object(arg)?;
    serde_json::from_value(serde_json::Value::Object(params))
        .map_err(|error| ChannelError::error(format!("invalid arguments: {error}")))
}

trait IntoChannel<T> {
    fn chan(self) -> Result<T, ChannelError>;
}

impl<T> IntoChannel<T> for Result<T, SubagentsError> {
    fn chan(self) -> Result<T, ChannelError> {
        self.map_err(|error| ChannelError::error(error.message))
    }
}

fn json_value(value: &serde_json::Value) -> Value {
    match serde_json::to_string(value) {
        Ok(text) => Value::Json(text),
        Err(error) => Value::Json(format!("{{\"serializeError\":\"{error}\"}}")),
    }
}
