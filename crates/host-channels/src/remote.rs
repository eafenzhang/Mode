//! `remote` 频道（LT-16.4d）：远程 workspace 的连接管理面。
//!
//! 真源：`packages/services/src/remote/remoteService.ts`（`IRemoteService`）。
//! 规范见 `docs/specs/remote-workspaces-channel.md`。
//!
//! 本频道只做「RPC 面」：把 8 个方法映射到 [`RemoteSessionRegistry`] 的状态机，
//! 真实建连由 [`RemoteTransportPort`] 注入（LAN / SSH / WSL / Docker / v3）。
//! 端口未装配时 `connect` 返回 `{ok:false,error}`——与 TS 同形，
//! **不抛错**（renderer 按返回值分支，抛错会走成未捕获异常）。
//!
//! `attach` / `detach` 依赖「把一整套服务集合注册进 scoped `ChannelServer`」，
//! 壳内尚无该能力，因此**显式报错**而不是返回假 id。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use remote_session_registry::{
    derive_workspace_identity, ConnectPlan, ConnectRequest, RegistryEffect, RegistryError,
    RemoteSessionRegistry, RemoteSessionSnapshot, RemoteTargetSpec, RemoteWorkspaceContext,
};
use rpc_host::{
    BoxFuture, CancellationToken, ChannelError, EventCallback, ServerChannel, Subscription,
};
use rpc_wire::Value as WireValue;
use serde::Deserialize;
use serde_json::{json, Value as Json};

use crate::params::first_arg_object;

pub const REMOTE_CHANNEL: &str = "remote";
pub const ON_DYNAMIC_CONNECTION_EVENT: &str = "onDynamicConnectionEvent";

/// 传输端口：真实建连 / 断连 / workspace 运行时释放。
///
/// 全部按 `entry_id` 记账——注册表用它对账，端口用同一 id 找自己的连接。
/// 端口实现负责在完成后回调 [`RemoteChannel`] 的 `complete`/`fail`。
pub trait RemoteTransportPort: Send + Sync {
    /// 起传输。返回 `Ok(())` 表示已经建好，频道会据此把 session 置 online。
    fn connect(&self, plan: ConnectPlan, target: RemoteTargetSpec)
        -> BoxFuture<Result<(), String>>;
    /// 中止尚未完成的传输（入口已退休，迟到结果必须丢弃）。
    fn abort(&self, entry_id: u64);
    /// 销毁一条连接。
    fn dispose(&self, entry_id: u64);
    /// 释放某个 workspace 的运行时（仅 WSL）。
    fn release_workspace(
        &self,
        entry_id: u64,
        workspace_key: String,
        generation: u32,
        context: RemoteWorkspaceContext,
    ) -> BoxFuture<Result<(), String>>;
}

type Listener = (String, EventCallback);

/// 动态连接事件总线（`onDynamicConnectionEvent`）。
///
/// TS 的语义是「只发给当前订阅者，不缓存、不重放」——与 `onDidChange` 那种幂等
/// 事实不同，所以这里不给首个订阅者补发任何东西。
#[derive(Default)]
pub struct ConnectionEventBus {
    listeners: Mutex<HashMap<String, Listener>>,
}

impl ConnectionEventBus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe(self: &Arc<Self>, emit: EventCallback) -> Subscription {
        let id = uuid::Uuid::new_v4().to_string();
        self.listeners
            .lock()
            .expect("connection listeners poisoned")
            .insert(id.clone(), (id.clone(), emit));
        let bus = Arc::clone(self);
        Box::new(move || {
            bus.listeners
                .lock()
                .expect("connection listeners poisoned")
                .remove(&id);
        })
    }

    pub fn emit(&self, payload: Json) {
        let callbacks: Vec<EventCallback> = self
            .listeners
            .lock()
            .expect("connection listeners poisoned")
            .values()
            .map(|(_, emit)| Arc::clone(emit))
            .collect();
        let text = payload.to_string();
        for emit in callbacks {
            emit(WireValue::Json(text.clone()));
        }
    }
}

pub struct RemoteChannel {
    registry: Arc<Mutex<RemoteSessionRegistry>>,
    events: Arc<ConnectionEventBus>,
    transport: Option<Arc<dyn RemoteTransportPort>>,
}

impl RemoteChannel {
    pub fn new(transport: Option<Arc<dyn RemoteTransportPort>>) -> Self {
        Self {
            registry: Arc::new(Mutex::new(RemoteSessionRegistry::new())),
            events: Arc::new(ConnectionEventBus::new()),
            transport,
        }
    }

    pub fn with_registry(
        registry: RemoteSessionRegistry,
        transport: Option<Arc<dyn RemoteTransportPort>>,
    ) -> Self {
        Self {
            registry: Arc::new(Mutex::new(registry)),
            events: Arc::new(ConnectionEventBus::new()),
            transport,
        }
    }
}

impl ServerChannel for RemoteChannel {
    fn call(
        &self,
        _ctx: String,
        command: String,
        arg: WireValue,
        _cancel: CancellationToken,
    ) -> BoxFuture<Result<WireValue, ChannelError>> {
        let transport = self.transport.clone();
        let registry = Arc::clone(&self.registry);
        let events = Arc::clone(&self.events);
        Box::pin(async move {
            match command.as_str() {
                "connect" => connect(registry, events, transport, arg).await,
                "cancelConnect" => {
                    let params: SessionIdParams = parse(&arg, "cancelConnect")?;
                    let effects = {
                        let mut guard = registry.lock().expect("registry poisoned");
                        guard.cancel_connect(&params.request_id)
                    };
                    apply_effects(registry, transport, effects).await;
                    Ok(WireValue::Undefined)
                }
                "bindContext" => {
                    let params: BindContextParams = parse(&arg, "bindContext")?;
                    if params.workspace_path.trim().is_empty() {
                        return Err(ChannelError::error("workspacePath 不能为空"));
                    }
                    let effects = {
                        let mut guard = registry.lock().expect("registry poisoned");
                        guard
                            .bind_context(
                                &params.session_id,
                                params.workspace_path.trim(),
                                params.workspace_identity.as_deref().unwrap_or("").trim(),
                            )
                            .map_err(registry_error)?
                    };
                    apply_effects(registry, transport, effects).await;
                    Ok(WireValue::Undefined)
                }
                "dispose" => {
                    let params: SessionParams = parse(&arg, "dispose")?;
                    let effects = {
                        let mut guard = registry.lock().expect("registry poisoned");
                        guard.dispose_session(&params.session_id)
                    };
                    apply_effects(registry, transport, effects).await;
                    Ok(WireValue::Undefined)
                }
                "getSession" => {
                    let params: SessionParams = parse(&arg, "getSession")?;
                    let snapshot = {
                        let guard = registry.lock().expect("registry poisoned");
                        guard.get_session(&params.session_id)
                    };
                    json_text(match snapshot {
                        Some(snapshot) => snapshot_value(&snapshot),
                        None => Json::Null,
                    })
                }
                "listSessions" => {
                    let snapshots = {
                        let guard = registry.lock().expect("registry poisoned");
                        guard.list_sessions()
                    };
                    json_text(Json::Array(snapshots.iter().map(snapshot_value).collect()))
                }
                "attach" | "detach" => Err(ChannelError::error(format!(
                    "remote channel: {command} 需要 scoped 服务集合注册能力（壳内尚未装配）"
                ))),
                other => Err(ChannelError::error(format!(
                    "remote channel: unknown command {other}"
                ))),
            }
        })
    }

    fn listen(
        &self,
        _ctx: String,
        event: String,
        _arg: WireValue,
        emit: EventCallback,
    ) -> Result<Subscription, ChannelError> {
        if event != ON_DYNAMIC_CONNECTION_EVENT {
            return Err(ChannelError::error(format!(
                "remote channel: unknown event {event}"
            )));
        }
        Ok(self.events.subscribe(emit))
    }
}

async fn connect(
    registry: Arc<Mutex<RemoteSessionRegistry>>,
    events: Arc<ConnectionEventBus>,
    transport: Option<Arc<dyn RemoteTransportPort>>,
    arg: WireValue,
) -> Result<WireValue, ChannelError> {
    let params: ConnectParams = parse(&arg, "connect")?;
    let target = params.target.into_spec()?;
    // identity 缺省时按统一构造器派生（业务代码不得手写拼接）。
    let workspace_identity = params
        .workspace_identity
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            params
                .workspace_path
                .as_deref()
                .map(|path| derive_workspace_identity(path, &target))
        });
    let request_id = params
        .request_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let (plan, effects) = {
        let mut guard = registry.lock().expect("registry poisoned");
        guard
            .begin_connect(ConnectRequest {
                request_id: request_id.clone(),
                target: target.clone(),
                workspace_path: params.workspace_path.clone(),
                workspace_identity: workspace_identity.clone(),
            })
            .map_err(registry_error)?
    };

    let descriptor = json!({
        "remoteSessionId": plan.remote_session_id,
        "target": target_descriptor(&target),
        "workspacePath": params.workspace_path,
        "workspaceIdentity": workspace_identity,
        "generation": 1,
    });

    let Some(port) = transport else {
        // 与 TS 同形：端口缺失时返回可读错误而不是抛（renderer 按 ok 分支处理）。
        let message = "远程工作区服务尚未就绪。".to_string();
        {
            let mut guard = registry.lock().expect("registry poisoned");
            let _ = guard.fail_connect(plan.entry_id);
        }
        events.emit(json!({
            "kind": "connectFailed",
            "requestId": request_id,
            "error": message,
        }));
        return json_text(json!({ "ok": false, "error": message }));
    };

    // 起传输（失败要把 session 一起收掉，否则会留下永远 connecting 的条目）。
    let connect_effect = effects.iter().find_map(|effect| match effect {
        RegistryEffect::Connect { .. } => Some(effect.clone()),
        _ => None,
    });
    if let Some(RegistryEffect::Connect {
        entry_id,
        connection_key: _,
        target: effect_target,
    }) = connect_effect
    {
        match port.connect(plan.clone(), effect_target).await {
            Ok(()) => {
                let follow = {
                    let mut guard = registry.lock().expect("registry poisoned");
                    guard.complete_connect(entry_id)
                };
                apply_effects(Arc::clone(&registry), Some(Arc::clone(&port)), follow).await;
                events.emit(json!({ "kind": "connected", "descriptor": descriptor }));
                return json_text(json!({ "ok": true, "descriptor": descriptor }));
            }
            Err(message) => {
                {
                    let mut guard = registry.lock().expect("registry poisoned");
                    let _ = guard.fail_connect(entry_id);
                }
                events.emit(json!({
                    "kind": "connectFailed",
                    "requestId": request_id,
                    "error": message,
                }));
                return json_text(json!({ "ok": false, "error": message }));
            }
        }
    }

    // 复用已 online 的 entry：无需再起传输。
    events.emit(json!({ "kind": "connected", "descriptor": descriptor }));
    json_text(json!({ "ok": true, "descriptor": descriptor }))
}

/// 执行注册表下发的副作用；每个 effect 都要回执，否则代际状态会卡住。
async fn apply_effects(
    registry: Arc<Mutex<RemoteSessionRegistry>>,
    transport: Option<Arc<dyn RemoteTransportPort>>,
    effects: Vec<RegistryEffect>,
) {
    for effect in effects {
        match effect {
            RegistryEffect::ReleaseWorkspace {
                entry_id,
                workspace_key,
                generation,
                context,
            } => {
                if let Some(port) = &transport {
                    let _ = port
                        .release_workspace(entry_id, workspace_key.clone(), generation, context)
                        .await;
                }
                let mut guard = registry.lock().expect("registry poisoned");
                let _ = guard.complete_workspace_release(entry_id, &workspace_key, generation);
            }
            RegistryEffect::AbortTransport { entry_id } => {
                if let Some(port) = &transport {
                    port.abort(entry_id);
                }
            }
            RegistryEffect::DisposeEntry { entry_id } => {
                if let Some(port) = &transport {
                    port.dispose(entry_id);
                }
            }
            // idle 销毁由端口自己排定时器（注册表只表达意愿，不持有时钟）。
            RegistryEffect::ScheduleIdleDispose { entry_id } => {
                if let Some(port) = &transport {
                    port.dispose(entry_id);
                }
            }
            RegistryEffect::ClearIdleTimer { .. } => {}
            RegistryEffect::Connect { .. } => {}
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectParams {
    target: TargetParams,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    workspace_path: Option<String>,
    #[serde(default)]
    workspace_identity: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum TargetParams {
    Ssh {
        host: String,
        #[serde(default)]
        port: Option<u16>,
        username: String,
        #[serde(default)]
        private_key_path: Option<String>,
        #[serde(default)]
        password: Option<String>,
    },
    Wsl {
        #[serde(default)]
        distro: Option<String>,
        #[serde(default)]
        user: Option<String>,
    },
    Docker {
        container: String,
    },
    Lan {
        host: String,
        port: u16,
    },
    V3 {
        device_id: String,
    },
}

impl TargetParams {
    fn into_spec(self) -> Result<RemoteTargetSpec, ChannelError> {
        Ok(match self {
            Self::Ssh {
                host,
                port,
                username,
                private_key_path,
                password,
            } => {
                let host = host.trim();
                let username = username.trim();
                if host.is_empty() || username.is_empty() {
                    return Err(ChannelError::error("ssh 目标缺少 host 或 username"));
                }
                RemoteTargetSpec::Ssh {
                    host: host.to_string(),
                    port,
                    username: username.to_string(),
                    private_key_path,
                    // 密码本身不进结构：只把「有没有密码」交给共享键的 authKind 判定。
                    has_password: password
                        .map(|value| !value.trim().is_empty())
                        .unwrap_or(false),
                }
            }
            Self::Wsl { distro, user } => RemoteTargetSpec::Wsl { distro, user },
            Self::Docker { container } => {
                let container = container.trim();
                if container.is_empty() {
                    return Err(ChannelError::error("docker 目标缺少 container"));
                }
                RemoteTargetSpec::Docker {
                    container: container.to_string(),
                }
            }
            Self::Lan { host, port } => {
                let host = host.trim();
                if host.is_empty() {
                    return Err(ChannelError::error("lan 目标缺少 host"));
                }
                RemoteTargetSpec::Lan {
                    host: host.to_string(),
                    port,
                }
            }
            Self::V3 { device_id } => {
                let device_id = device_id.trim();
                if device_id.is_empty() {
                    return Err(ChannelError::error("v3 目标缺少 deviceId"));
                }
                RemoteTargetSpec::V3 {
                    device_id: device_id.to_string(),
                }
            }
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionParams {
    session_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionIdParams {
    request_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BindContextParams {
    session_id: String,
    workspace_path: String,
    #[serde(default)]
    workspace_identity: Option<String>,
}

/// 快照 → 线上形状（`RemoteWorkspaceSessionSnapshot`）。
fn snapshot_value(snapshot: &RemoteSessionSnapshot) -> Json {
    json!({
        "remoteSessionId": snapshot.remote_session_id,
        "requestId": snapshot.request_id,
        "target": target_descriptor(&snapshot.target),
        "workspacePath": snapshot.workspace_path,
        "workspaceIdentity": snapshot.workspace_identity,
        "generation": snapshot.generation,
        "state": snapshot.state.as_str(),
        "sourceAvailability": snapshot.source_availability,
    })
}

/// target 的线上形状：**secret 一律不下发**（token/密码/口令）。
fn target_descriptor(target: &RemoteTargetSpec) -> Json {
    match target {
        RemoteTargetSpec::Ssh {
            host,
            port,
            username,
            private_key_path,
            ..
        } => {
            let mut value = json!({
                "kind": "ssh",
                "host": host,
                "port": port.unwrap_or(22),
                "username": username,
            });
            if let Some(path) = private_key_path {
                value["privateKeyPath"] = json!(path);
            }
            value
        }
        RemoteTargetSpec::Wsl { distro, user } => json!({
            "kind": "wsl",
            "distro": distro,
            "user": user,
        }),
        RemoteTargetSpec::Docker { container } => {
            json!({ "kind": "docker", "container": container })
        }
        RemoteTargetSpec::Lan { host, port } => {
            json!({ "kind": "lan", "host": host, "port": port })
        }
        RemoteTargetSpec::V3 { device_id } => {
            json!({ "kind": "v3", "deviceId": device_id })
        }
    }
}

fn parse<T: serde::de::DeserializeOwned>(
    arg: &WireValue,
    command: &str,
) -> Result<T, ChannelError> {
    let object = first_arg_object(arg)?;
    serde_json::from_value(Json::Object(object))
        .map_err(|error| ChannelError::error(format!("invalid {command} arguments: {error}")))
}

fn registry_error(error: RegistryError) -> ChannelError {
    ChannelError::error(error.to_string())
}

fn json_text(value: Json) -> Result<WireValue, ChannelError> {
    serde_json::to_string(&value)
        .map(WireValue::Json)
        .map_err(|error| ChannelError::error(error.to_string()))
}
