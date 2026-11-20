//! Workflows 方法表（C4 T3）：list / get / updateMeta / delete / move / runs。
//!
//! 文件与 run 读面归 `crate::workflows`；本层只做严格参数与数据根解析。
//! `v4/command startSavedWorkflow` 的脚本执行不在 Rust（无 JS 引擎，契约 §4.6 登记）。

use std::path::PathBuf;

use agent_protocol::workflow::{
    WorkflowsDeleteParams, WorkflowsGetParams, WorkflowsListParams, WorkflowsMoveParams,
    WorkflowsRunsParams, WorkflowsUpdateMetaParams,
};
use serde_json::Value;

use crate::dispatch::AgentServer;
use crate::server::ServerError;
use crate::workflows::{self, WorkflowDirs};

use super::parse_params;

fn data_root(server: &AgentServer) -> Result<PathBuf, ServerError> {
    server
        .data_root()
        .ok_or_else(|| ServerError::internal("数据根不可用：workflows 无事实源"))
}

fn dirs(server: &AgentServer, workspace_path: &str) -> Result<WorkflowDirs, ServerError> {
    Ok(WorkflowDirs::new(workspace_path, &data_root(server)?))
}

fn to_value<T: serde::Serialize>(value: T) -> Result<Value, ServerError> {
    serde_json::to_value(value).map_err(|error| ServerError::internal(error.to_string()))
}

pub(crate) fn handle_list(server: &AgentServer, params: Value) -> Result<Value, ServerError> {
    let params: WorkflowsListParams = parse_params(crate::methods::WORKFLOWS_LIST_METHOD, params)?;
    let dirs = dirs(server, &params.workspace.workspace_path)?;
    to_value(workflows::list(&dirs, params.scope))
}

pub(crate) fn handle_get(server: &AgentServer, params: Value) -> Result<Value, ServerError> {
    let params: WorkflowsGetParams = parse_params(crate::methods::WORKFLOWS_GET_METHOD, params)?;
    let dirs = dirs(server, &params.workspace.workspace_path)?;
    to_value(workflows::get(&dirs, &params.name, params.scope))
}

pub(crate) fn handle_update_meta(
    server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: WorkflowsUpdateMetaParams =
        parse_params(crate::methods::WORKFLOWS_UPDATE_META_METHOD, params)?;
    let dirs = dirs(server, &params.workspace.workspace_path)?;
    to_value(workflows::update_meta(
        &dirs,
        &params.name,
        &params.meta,
        params.scope,
    ))
}

pub(crate) fn handle_delete(server: &AgentServer, params: Value) -> Result<Value, ServerError> {
    let params: WorkflowsDeleteParams =
        parse_params(crate::methods::WORKFLOWS_DELETE_METHOD, params)?;
    let dirs = dirs(server, &params.workspace.workspace_path)?;
    to_value(workflows::delete(&dirs, &params.name, params.scope))
}

pub(crate) fn handle_move(server: &AgentServer, params: Value) -> Result<Value, ServerError> {
    let params: WorkflowsMoveParams = parse_params(crate::methods::WORKFLOWS_MOVE_METHOD, params)?;
    let dirs = dirs(server, &params.workspace.workspace_path)?;
    to_value(workflows::move_workflow(&dirs, &params.name))
}

pub(crate) fn handle_runs(server: &AgentServer, params: Value) -> Result<Value, ServerError> {
    let params: WorkflowsRunsParams = parse_params(crate::methods::WORKFLOWS_RUNS_METHOD, params)?;
    if params.limit == 0 || params.limit > agent_protocol::workflow::WORKFLOWS_RUNS_MAX_LIMIT {
        return Err(ServerError::invalid_params("limit 必须是 1..=50 的整数"));
    }
    let data_root = data_root(server)?;
    to_value(workflows::runs(
        &data_root,
        &params.workspace.workspace_path,
        params.name.as_deref(),
        params.limit,
    ))
}
