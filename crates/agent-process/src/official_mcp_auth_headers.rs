//! `interaction/requestOfficialMcpAuthHeaders` 宿主应答。
//!
//! 官方 Server MCP 身份权威边界：Agent 报 (pluginId, mcpKey, targetOrigin)，
//! Host 校验 origin 信任并回当前身份头。失败原因必须可枚举（协议层三类）。

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use agent_protocol::official_mcp_auth::{
    OfficialMcpAuthHeadersRequestParams, OfficialMcpAuthHeadersResponse,
    OfficialMcpAuthPortFailureReason, OFFICIAL_MCP_AUTH_HEADERS_METHOD,
};
use mode_protocol_client::{ReplyHandle, RequestHandler, RequestReply};
use serde_json::Value;

pub type OfficialMcpAuthHeadersFuture<'a> =
    Pin<Box<dyn Future<Output = Result<BTreeMap<String, String>, OfficialMcpAuthPortFailureReason>> + Send + 'a>>;

/// 解析当前官方 Server MCP 身份头；失败返回协议层枚举原因。
pub trait OfficialMcpAuthHeadersResolver: Send + Sync {
    fn resolve<'a>(
        &'a self,
        params: &'a OfficialMcpAuthHeadersRequestParams,
    ) -> OfficialMcpAuthHeadersFuture<'a>;
}

pub fn official_mcp_auth_headers_handler(
    resolver: Arc<dyn OfficialMcpAuthHeadersResolver>,
    runtime: tokio::runtime::Handle,
) -> RequestHandler {
    Arc::new(move |request: &Value, reply: ReplyHandle| {
        if request.get("method").and_then(Value::as_str)
            != Some(OFFICIAL_MCP_AUTH_HEADERS_METHOD)
        {
            return RequestReply::NotHandled;
        }
        let Some(params) = request
            .get("params")
            .cloned()
            .and_then(|value| {
                serde_json::from_value::<OfficialMcpAuthHeadersRequestParams>(value).ok()
            })
        else {
            reply.respond(RequestReply::Error {
                code: -32602,
                message: "Invalid official MCP auth headers request params".to_string(),
            });
            return RequestReply::Deferred;
        };
        let resolver = Arc::clone(&resolver);
        runtime.spawn(async move {
            let response = match resolver.resolve(&params).await {
                Ok(headers) => OfficialMcpAuthHeadersResponse::Ok { headers },
                Err(reason) => OfficialMcpAuthHeadersResponse::Failed { reason },
            };
            match serde_json::to_value(response) {
                Ok(value) => reply.respond(RequestReply::Result(value)),
                Err(error) => reply.respond(RequestReply::Error {
                    code: -32603,
                    message: error.to_string(),
                }),
            }
        });
        RequestReply::Deferred
    })
}
