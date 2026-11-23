//! 远程频道代理：把一个**远端**频道（经 LAN 连接）暴露成本地的 `ServerChannel`。
//!
//! 真源：`packages/rpc/src/proxy-channel.ts` 的 `ProxyChannel`（服务端那一半）。
//!
//! 为什么需要它：`attach` 要在新的 scope 上注册「该 remote workspace 的服务面」，
//! 而这些服务**不在本进程**——它们在远端 Host 里。本地这一侧只做转发：
//! 每个频道名一个代理，把 `call`/`listen` 原样发给 [`ChannelClient`]。
//!
//! 三条语义：
//!
//! 1. **参数是位置数组**：`ProxyChannel` 约定 `arg` 是数组（对象在首位），
//!    远端收到的东西必须与 renderer 直接连远端时**逐字节同形**。这里不拆包、不重组。
//! 2. **错误要保留形状**：远端的 `PromiseError` 带 `message`/`name`/透传字段，
//!    本地必须还原成 `ChannelError::Error`，否则 renderer 拿不到 `code`。
//! 3. **订阅释放要转发**：本地 `listen` 返回的释放函数必须让远端发 `EventDispose`，
//!    否则 scope 回收后远端订阅永久泄漏。

use std::sync::Arc;

use rpc_client::{ChannelClient, ClientError};
use rpc_host::{
    BoxFuture, CancellationToken, ChannelError, EventCallback, ServerChannel, Subscription,
};
use rpc_wire::Value;
use tokio::sync::oneshot;

/// 一个远端频道在本地的代理。
pub struct RemoteProxyChannel {
    client: Arc<ChannelClient>,
    channel: String,
}

impl RemoteProxyChannel {
    pub fn new(client: Arc<ChannelClient>, channel: impl Into<String>) -> Self {
        Self {
            client,
            channel: channel.into(),
        }
    }
}

impl ServerChannel for RemoteProxyChannel {
    fn call(
        &self,
        _ctx: String,
        command: String,
        arg: Value,
        cancel: CancellationToken,
    ) -> BoxFuture<Result<Value, ChannelError>> {
        let client = Arc::clone(&self.client);
        let channel = self.channel.clone();
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(ChannelError::error("Cancelled"));
            }
            let (tx, rx) = oneshot::channel::<Result<Value, ClientError>>();
            let tx = Arc::new(std::sync::Mutex::new(Some(tx)));
            let (id, _token) = client.call(
                &channel,
                &command,
                arg,
                Arc::new(move |result| {
                    if let Some(tx) = tx.lock().expect("proxy responder").take() {
                        let _ = tx.send(result);
                    }
                }),
            );
            // 本地取消要传导到远端（否则远端会一直等到超时）。
            if cancel.is_cancelled() {
                client.cancel(id);
            }
            match rx.await {
                Ok(Ok(value)) => Ok(value),
                Ok(Err(error)) => Err(ChannelError::from(error)),
                // responder 被丢弃（客户端 dispose）：按连接关闭收口，不能挂起。
                Err(_) => Err(ChannelError::error("远程连接已关闭")),
            }
        })
    }

    fn listen(
        &self,
        _ctx: String,
        event: String,
        _arg: Value,
        emit: EventCallback,
    ) -> Result<Subscription, ChannelError> {
        // 订阅释放必须转发：否则远端在 scope 回收后仍持有该订阅。
        let dispose = self.client.listen(&self.channel, &event, emit);
        Ok(dispose)
    }
}
