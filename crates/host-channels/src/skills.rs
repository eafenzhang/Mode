//! `skills` 频道适配器：6 个方法。
//!
//! 参数形状是 `ProxyChannel` 的位置参数数组（第一个元素为对象）；服务是同步实现
//! （目录扫描 + 小文件读写），统一丢进 `spawn_blocking`，避免阻塞 RPC 执行器。
//!
//! 行为契约见 `docs/specs/skills-service.md`。

use std::path::PathBuf;
use std::sync::Arc;

use rpc_host::{BoxFuture, CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;
use serde::Deserialize;
use skills_service::{
    ListParams, PromptContextParams, SetEnabledParams, SkillIdParams, SkillsService,
    SkillsServiceError,
};

use crate::params::first_arg_object;

pub const LIST: &str = "list";
pub const SET_ENABLED: &str = "setEnabled";
pub const BUILD_PROMPT_CONTEXT: &str = "buildPromptContext";
pub const COPY_TO_COMMON: &str = "copyToCommon";
pub const REMOVE_FROM_COMMON: &str = "removeFromCommon";
pub const DELETE_SKILL: &str = "deleteSkill";

pub struct SkillsChannel {
    service: Arc<SkillsService>,
}

impl SkillsChannel {
    pub fn new(service: Arc<SkillsService>) -> Self {
        Self { service }
    }
}

impl ServerChannel for SkillsChannel {
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
                    ChannelError::error(format!("skills channel task failed: {error}"))
                })?
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListArgs {
    workspace_path: PathBuf,
    #[serde(default)]
    workspace_identity: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetEnabledArgs {
    workspace_path: PathBuf,
    #[serde(default)]
    workspace_identity: Option<String>,
    skill_id: String,
    enabled: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PromptContextArgs {
    workspace_path: PathBuf,
    #[serde(default)]
    workspace_identity: Option<String>,
    prompt: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkillIdArgs {
    workspace_path: PathBuf,
    #[serde(default)]
    workspace_identity: Option<String>,
    skill_id: String,
}

fn run(service: &SkillsService, command: &str, arg: &Value) -> Result<Value, ChannelError> {
    match command {
        LIST => {
            let args: ListArgs = parse(arg)?;
            Ok(json(&service.list(&ListParams {
                workspace_path: args.workspace_path,
                workspace_identity: args.workspace_identity,
            })))
        }
        SET_ENABLED => {
            let args: SetEnabledArgs = parse(arg)?;
            service
                .set_enabled(&SetEnabledParams {
                    workspace_path: args.workspace_path,
                    workspace_identity: args.workspace_identity,
                    skill_id: args.skill_id,
                    enabled: args.enabled,
                })
                .chan()?;
            Ok(Value::Undefined)
        }
        BUILD_PROMPT_CONTEXT => {
            let args: PromptContextArgs = parse(arg)?;
            Ok(json(
                &service
                    .build_prompt_context(&PromptContextParams {
                        workspace_path: args.workspace_path,
                        workspace_identity: args.workspace_identity,
                        prompt: args.prompt,
                    })
                    .chan()?,
            ))
        }
        COPY_TO_COMMON => {
            let args: SkillIdArgs = parse(arg)?;
            Ok(json(
                &service
                    .copy_to_common(&SkillIdParams {
                        workspace_path: args.workspace_path,
                        workspace_identity: args.workspace_identity,
                        skill_id: args.skill_id,
                    })
                    .chan()?,
            ))
        }
        REMOVE_FROM_COMMON => {
            let args: SkillIdArgs = parse(arg)?;
            service
                .remove_from_common(&SkillIdParams {
                    workspace_path: args.workspace_path,
                    workspace_identity: args.workspace_identity,
                    skill_id: args.skill_id,
                })
                .chan()?;
            Ok(Value::Undefined)
        }
        DELETE_SKILL => {
            let args: SkillIdArgs = parse(arg)?;
            service
                .delete_skill(&SkillIdParams {
                    workspace_path: args.workspace_path,
                    workspace_identity: args.workspace_identity,
                    skill_id: args.skill_id,
                })
                .chan()?;
            Ok(Value::Undefined)
        }
        other => Err(ChannelError::error(format!(
            "skills channel: unknown command {other}"
        ))),
    }
}

fn parse<T: serde::de::DeserializeOwned>(arg: &Value) -> Result<T, ChannelError> {
    let params = first_arg_object(arg)?;
    serde_json::from_value(serde_json::Value::Object(params))
        .map_err(|error| ChannelError::error(format!("invalid arguments: {error}")))
}

/// `SkillsServiceError` → `ChannelError`（跨 crate 无法实现 `From`，用局部扩展 trait）。
trait IntoChannel<T> {
    fn chan(self) -> Result<T, ChannelError>;
}

impl<T> IntoChannel<T> for Result<T, SkillsServiceError> {
    fn chan(self) -> Result<T, ChannelError> {
        self.map_err(|error| ChannelError::error(error.to_string()))
    }
}

fn json(value: &serde_json::Value) -> Value {
    match serde_json::to_string(value) {
        Ok(text) => Value::Json(text),
        Err(error) => Value::Json(format!("{{\"serializeError\":\"{error}\"}}")),
    }
}
