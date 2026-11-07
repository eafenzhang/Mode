//! `interaction/requestProviderRuntimeHeaders` 宿主应答。
//!
//! Agent 在账号 Provider 模型调用前请求鉴权材料；Host 解析当前账号事实并回
//! `{headersApplied, requestAuth}`。解析是异步的，因此处理器在注入的运行时上
//! spawn 并用 `ReplyHandle` 延迟应答。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use agent_protocol::provider_runtime_headers::{
    ProviderRuntimeHeadersRequestParams, ProviderRuntimeHeadersResponse,
    PROVIDER_RUNTIME_HEADERS_METHOD,
};
use mode_protocol_client::{ReplyHandle, RequestHandler, RequestReply};
use serde_json::Value;

pub type ProviderRuntimeHeadersFuture<'a> =
    Pin<Box<dyn Future<Output = Option<String>> + Send + 'a>>;

/// 解析当前账号请求凭据；返回 `None` 时回 `headersApplied:false`。
pub trait ProviderRuntimeHeadersResolver: Send + Sync {
    fn resolve<'a>(
        &'a self,
        params: &'a ProviderRuntimeHeadersRequestParams,
    ) -> ProviderRuntimeHeadersFuture<'a>;
}

pub fn provider_runtime_headers_handler(
    resolver: Arc<dyn ProviderRuntimeHeadersResolver>,
    runtime: tokio::runtime::Handle,
) -> RequestHandler {
    Arc::new(move |request: &Value, reply: ReplyHandle| {
        if request.get("method").and_then(Value::as_str) != Some(PROVIDER_RUNTIME_HEADERS_METHOD) {
            return RequestReply::NotHandled;
        }
        let Some(params) = request
            .get("params")
            .cloned()
            .and_then(|value| serde_json::from_value::<ProviderRuntimeHeadersRequestParams>(value).ok())
        else {
            reply.respond(RequestReply::Error {
                code: -32602,
                message: "Invalid provider runtime headers request params".to_string(),
            });
            return RequestReply::Deferred;
        };
        let resolver = Arc::clone(&resolver);
        runtime.spawn(async move {
            let response = match resolver.resolve(&params).await {
                Some(api_key) => ProviderRuntimeHeadersResponse::applied(api_key),
                None => ProviderRuntimeHeadersResponse::not_applied(
                    "Account request credential is unavailable",
                ),
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
