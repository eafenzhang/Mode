//! 远端服务面工厂：把一个 remote session 的**远端**频道集合注册成 scoped 服务面。
//!
//! 真源：`packages/desktop/src/host/remoteWorkspaceService.ts` 的 `attach`——
//! `remoteServices.exposeOnChannelServer(guardedServer)` 把远端服务集合挂到新 scope 上。
//!
//! Rust 侧的实现路径不同：远端服务**不在本进程**，所以每个频道名挂一个
//! [`RemoteProxyChannel`]，由它把 `call`/`listen` 转发给 [`ChannelClient`]。
//!
//! 为什么频道名要显式给：远端 Host 跑的是同一份 `assembly`，所以
//! `service_channel_names()` 就是它的服务面；把这份名单显式传进来，
//! 既避免硬编码，也让「远端多了一个频道」在装配处一目了然。

use std::sync::Arc;

use remote_session_registry::RemoteSessionSnapshot;
use rpc_client::ChannelClient;
use rpc_host::ChannelServer;

use crate::remote::RemoteScopeFactory;
use crate::remote_proxy::RemoteProxyChannel;

/// 按频道名列表挂远端代理的工厂。
pub struct RemoteServicesScopeFactory {
    channel_names: Vec<String>,
}

impl RemoteServicesScopeFactory {
    pub fn new(channel_names: Vec<String>) -> Self {
        Self { channel_names }
    }

    /// 用本地 assembly 的服务面名单构造（远端跑同一份装配）。
    pub fn for_local_service_surface() -> Self {
        Self::new(
            crate::assembly::service_channel_names()
                .into_iter()
                .map(str::to_string)
                .collect(),
        )
    }
}

impl RemoteScopeFactory for RemoteServicesScopeFactory {
    fn register(
        &self,
        server: &mut ChannelServer,
        _session: &RemoteSessionSnapshot,
        client: Option<Arc<ChannelClient>>,
    ) -> Result<(), String> {
        // 没有客户端就没有远端服务面可代理：显式报错，不挂空代理
        // （空代理会让 renderer 的每个 RPC 都等到超时才失败）。
        let client = client.ok_or_else(|| {
            "该远程传输在壳内没有 RPC 客户端，无法提供服务面（SSH/WSL/Docker/v3 未落地）"
                .to_string()
        })?;
        for name in &self.channel_names {
            server.register_channel(
                name.clone(),
                Arc::new(RemoteProxyChannel::new(Arc::clone(&client), name.clone())),
            );
        }
        Ok(())
    }
}
