//! `client-config` 与 `client-scenes` 适配器（当前构建下是常量服务）。
//!
//! 真源：
//! - `packages/services/src/client-config/clientConfigService.ts`：`isOfficialServiceEnabled`
//!   为 false 时**在凭证读取与网络请求之前**短路，返回 `{ pluginStoreOrder: null }`；
//! - `packages/services/src/client-scenes/clientScenesService.ts`：审计版无远端场景，
//!   返回 `{ code: 0, msg: "", data: [] }`。
//!
//! `packages/shared/src/officialPlatformPolicy.ts` 的 `isOfficialServiceEnabled` 目前恒为
//! `false`。这里如实复制该分支，**不实现网络路径**；若官方服务重新启用，这两条适配器必须
//! 与 TS 同步改造（spec `host-channel-adapters.md` 记录了这条约束）。

use rpc_host::{BoxFuture, CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;

pub const GET_SNAPSHOT: &str = "getSnapshot";
pub const LIST: &str = "list";

pub struct ClientConfigChannel;

impl ServerChannel for ClientConfigChannel {
    fn call(
        &self,
        _ctx: String,
        command: String,
        _arg: Value,
        _cancel: CancellationToken,
    ) -> BoxFuture<Result<Value, ChannelError>> {
        Box::pin(async move {
            match command.as_str() {
                GET_SNAPSHOT => Ok(Value::Json("{\"pluginStoreOrder\":null}".into())),
                other => Err(ChannelError::error(format!(
                    "client-config: unknown command {other}"
                ))),
            }
        })
    }
}

pub struct ClientScenesChannel;

impl ServerChannel for ClientScenesChannel {
    fn call(
        &self,
        _ctx: String,
        command: String,
        _arg: Value,
        _cancel: CancellationToken,
    ) -> BoxFuture<Result<Value, ChannelError>> {
        Box::pin(async move {
            match command.as_str() {
                LIST => Ok(Value::Json("{\"code\":0,\"msg\":\"\",\"data\":[]}".into())),
                other => Err(ChannelError::error(format!(
                    "client-scenes: unknown command {other}"
                ))),
            }
        })
    }
}
