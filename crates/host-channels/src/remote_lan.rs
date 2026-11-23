//! LAN transport 端口：把 [`rpc_client::lan`] 接到 [`RemoteTransportPort`]。
//!
//! 规范见 `docs/specs/remote-workspaces-channel.md` §6。
//!
//! 端口只做三件事：**按 entry_id 记账连接**、把 `connect` 委派给 LAN 连接层、
//! 在 abort/dispose 时断开。它不解释业务——session 的 online/offline 由注册表决定。
//!
//! 四条容易写错的地方：
//!
//! 1. **令牌从凭据取，不从 target 抄**。`target.token` 只在连接流程内透明传递
//!    （TS `stripRemoteTargetSecrets` 同语义）；落盘与复用一律走凭据服务。
//! 2. **没有配对就没有令牌**：建连前直接报「这台设备还没有配对」，先建 WS 再失败会在
//!    对端留下一次失败鉴权。
//! 3. **abort 与 dispose 都可能命中同一 entry**：map 先摘再断，避免重复 close。
//! 4. **`entries` 用 `Arc<Mutex<…>>`**：`connect` 的 async 块是 `'static`，借不到
//!    `&self`；把 map 的 Arc 克隆进块里是唯一干净的做法（thread-local 交接位会在
//!    多线程 runtime 下丢连接）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use credential_store::CredentialStore;
use remote_session_registry::{ConnectPlan, RemoteTargetSpec, RemoteWorkspaceContext};
use rpc_client::lan::{attach_lan_remote_connection, LanAttachParams, LanRemoteConnection};
use rpc_host::BoxFuture;

use crate::lan_access::{peer_meta_key, peer_token_key, LanPeerMeta};
use crate::remote::RemoteTransportPort;

/// scoped 回帧的出站 sink（已带 `0xFF + scopeId` 前缀）。
pub type ScopedOutSink = Arc<dyn Fn(Vec<u8>) + Send + Sync>;

type Entries = Arc<Mutex<HashMap<u64, LanEntry>>>;

/// 一条 LAN 连接的运行期登记。
struct LanEntry {
    connection: LanRemoteConnection,
}

/// LAN transport 端口。
pub struct LanTransportPort {
    credentials: Arc<CredentialStore>,
    entries: Entries,
}

impl LanTransportPort {
    pub fn new(credentials: Arc<CredentialStore>) -> Self {
        Self {
            credentials,
            entries: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 当前活连接数（测试与诊断用）。
    pub fn active_entries(&self) -> usize {
        self.entries.lock().expect("lan entries poisoned").len()
    }

    /// 解析连接参数：显式 token > 凭据；端点优先取凭据里的 meta。
    fn resolve_params(&self, target: &RemoteTargetSpec) -> Result<LanAttachParams, String> {
        let RemoteTargetSpec::Lan {
            host,
            port,
            server_id,
            token,
        } = target
        else {
            return Err("LAN 端口只处理 lan 目标".to_string());
        };

        let stored = server_id.as_deref().and_then(|id| {
            let raw = self.credentials.load(&peer_meta_key(id)).ok().flatten();
            LanPeerMeta::parse(raw)
        });
        let stored_token = server_id
            .as_deref()
            .and_then(|id| self.credentials.load(&peer_token_key(id)).ok().flatten())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());

        let resolved_token = token
            .clone()
            .filter(|value| !value.trim().is_empty())
            .or(stored_token)
            .ok_or_else(|| "这台设备还没有配对：请先完成局域网配对".to_string())?;

        // 端点事实源：显式 target > 凭据 meta > target 的主端点。
        let (resolved_host, resolved_port, path, scheme) = match stored {
            Some(meta) if !meta.endpoints.is_empty() => {
                let primary = &meta.endpoints[0];
                (
                    primary.host.clone(),
                    primary.port,
                    primary.path.clone(),
                    Some(primary.scheme.clone()),
                )
            }
            _ => (host.clone(), *port, None, None),
        };

        Ok(LanAttachParams {
            host: resolved_host,
            port: resolved_port,
            scheme,
            path,
            token: resolved_token,
        })
    }
}

impl RemoteTransportPort for LanTransportPort {
    fn connect(
        &self,
        plan: ConnectPlan,
        target: RemoteTargetSpec,
    ) -> BoxFuture<Result<(), String>> {
        let params = match self.resolve_params(&target) {
            Ok(params) => params,
            // 配对缺失等确定性错误在**建连前**返回：不去打对端，避免失败鉴权噪声。
            Err(message) => return Box::pin(async move { Err(message) }),
        };
        let entry_id = plan.entry_id;
        let entries = Arc::clone(&self.entries);
        Box::pin(async move {
            let connection = attach_lan_remote_connection(params, None)
                .await
                .map_err(|error| error.to_string())?;
            entries
                .lock()
                .expect("lan entries poisoned")
                .insert(entry_id, LanEntry { connection });
            Ok(())
        })
    }

    fn channel_client(&self, entry_id: u64) -> Option<Arc<rpc_client::ChannelClient>> {
        self.entries
            .lock()
            .expect("lan entries poisoned")
            .get(&entry_id)
            .map(|entry| Arc::clone(&entry.connection.client))
    }

    fn abort(&self, entry_id: u64) {
        // 先摘再断：abort 与 dispose 命中同一 entry 时不重复 close。
        let entry = self
            .entries
            .lock()
            .expect("lan entries poisoned")
            .remove(&entry_id);
        if let Some(mut entry) = entry {
            entry.connection.dispose();
        }
    }

    fn dispose(&self, entry_id: u64) {
        let entry = self
            .entries
            .lock()
            .expect("lan entries poisoned")
            .remove(&entry_id);
        if let Some(mut entry) = entry {
            entry.connection.dispose();
        }
    }

    fn release_workspace(
        &self,
        _entry_id: u64,
        _workspace_key: String,
        _generation: u32,
        _context: RemoteWorkspaceContext,
    ) -> BoxFuture<Result<(), String>> {
        // LAN 对端是「已经在跑的 Mode 实例」，不部署也不启动任何东西，
        // 因此没有 workspace 运行时需要释放（TS 同：只有 wsl 有 release）。
        Box::pin(async { Ok(()) })
    }
}

impl Drop for LanTransportPort {
    fn drop(&mut self) {
        // 端口生命周期结束（Host 退出）：断开全部连接，避免 WS 悬挂。
        let entries = std::mem::take(&mut *self.entries.lock().expect("lan entries poisoned"));
        for (_, mut entry) in entries {
            entry.connection.dispose();
        }
    }
}
