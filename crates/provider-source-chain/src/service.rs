//! `AccountProviderService` 编排：配置源 → 账号 Resolver → Overlay 快照。
//!
//! 真源：`packages/provider/src/account-provider-service.ts`。关键语义：
//! - `read()` 缓存首份快照；无快照时共享同一轮 in-flight 结果；
//! - `refresh(reason)` 登记 generation，只有覆盖该 generation 的轮次才能回应；
//! - 进行中轮次开始后到达的请求留给下一轮；解析期间配置 revision 变化或又有新请求
//!   时整轮丢弃并补 `superseded-resolution`；
//! - 首次解析失败发布 fail-closed 快照，之后失败保留 last-known-good 并上报错误；
//! - 配置源变化在启动后订阅，失败只上报不改变 LKG。
//!
//! 网络/OAuth/凭据实现不在本模块；它们由注入的 Resolver 提供。

use std::sync::{Arc, Mutex, MutexGuard, Weak};

use account_provider_overlay::{
    create_account_provider_config_snapshot, create_fail_closed_account_provider_config_snapshot,
    AccountProviderConfigSnapshot, AccountProviderConfigSource, AccountProviderOverlayOptions,
    AccountProviderResolveInput, AccountProviderResolveOutput,
};
use provider_config::{AccountProviderStates, ProviderConfigMap};
use provider_config_source::{read_snapshot, ProviderConfigSnapshot};
use tokio::sync::{oneshot, watch};

const DISPOSED: &str = "AccountProviderService 已 dispose";
const NOT_STARTED: &str = "Account Provider Service 尚未产生快照";

pub type ServiceFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;
pub type Unsubscribe = Box<dyn FnOnce() + Send + 'static>;
type ChangeListener = Arc<dyn Fn(&str) + Send + Sync>;
type ErrorListener = Arc<dyn Fn(&RefreshErrorEvent) + Send + Sync>;

/// 配置源端口：与 TS `ProviderSource<ProviderConfigSnapshot>` 对齐。
pub trait ConfigSourcePort: Send + Sync {
    fn read(&self) -> ServiceFuture<'_, Result<ProviderConfigSnapshot, String>>;
    fn subscribe(&self, listener: ChangeListener) -> Unsubscribe;
}

/// 账号 Resolver 端口：与 TS `AccountProviderResolver` 对齐。
pub trait ResolverPort: Send + Sync {
    fn resolve(
        &self,
        request: ResolveRequest,
    ) -> ServiceFuture<'_, Result<AccountProviderResolveOutput, String>>;
}

/// Resolver 入参（TS 的 `AccountProviderResolveInput`，已拥有所有权以便跨轮传递）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveRequest {
    pub config_revision: String,
    pub configured_providers: ProviderConfigMap,
    pub previous_providers: ProviderConfigMap,
    pub previous_states: Option<AccountProviderStates>,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshErrorEvent {
    pub error: String,
    pub reasons: Vec<String>,
}

type Snapshot = AccountProviderConfigSnapshot;
type RoundOutcome = Result<Arc<Snapshot>, String>;

struct Waiter {
    generation: u64,
    sender: oneshot::Sender<RoundOutcome>,
}

struct Inner {
    snapshot: Option<Arc<Snapshot>>,
    pending_reasons: Vec<String>,
    requested_generation: u64,
    waiters: Vec<Waiter>,
    driver_active: bool,
    drive_result: Option<watch::Sender<Option<RoundOutcome>>>,
    started: bool,
    disposed: bool,
    unsubscribe: Option<Unsubscribe>,
}

impl Inner {
    fn push_reason(&mut self, reason: &str) {
        if !self.pending_reasons.iter().any(|value| value == reason) {
            self.pending_reasons.push(reason.to_owned());
        }
    }

    fn take_waiters(&mut self, generation: u64) -> Vec<oneshot::Sender<RoundOutcome>> {
        let mut keep = Vec::new();
        let mut ready = Vec::new();
        for waiter in self.waiters.drain(..) {
            if waiter.generation <= generation {
                ready.push(waiter.sender);
            } else {
                keep.push(waiter);
            }
        }
        self.waiters = keep;
        ready
    }
}

pub struct AccountProviderService {
    config_source: Arc<dyn ConfigSourcePort>,
    resolver: Arc<dyn ResolverPort>,
    inner: Mutex<Inner>,
    change_listeners: Arc<Mutex<Vec<ChangeListener>>>,
    error_listeners: Arc<Mutex<Vec<ErrorListener>>>,
    weak: Weak<AccountProviderService>,
}

impl AccountProviderService {
    pub fn new(
        config_source: Arc<dyn ConfigSourcePort>,
        resolver: Arc<dyn ResolverPort>,
    ) -> Arc<Self> {
        Arc::new_cyclic(|weak| Self {
            config_source,
            resolver,
            inner: Mutex::new(Inner {
                snapshot: None,
                pending_reasons: Vec::new(),
                requested_generation: 0,
                waiters: Vec::new(),
                driver_active: false,
                drive_result: None,
                started: false,
                disposed: false,
                unsubscribe: None,
            }),
            change_listeners: Arc::new(Mutex::new(Vec::new())),
            error_listeners: Arc::new(Mutex::new(Vec::new())),
            weak: weak.clone(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn assert_not_disposed(&self) -> Result<(), String> {
        if self.lock().disposed {
            Err(DISPOSED.to_owned())
        } else {
            Ok(())
        }
    }

    /// 上一次发布（成功或 fail-closed）的快照。
    pub fn last_snapshot(&self) -> Option<Arc<Snapshot>> {
        self.lock().snapshot.clone()
    }

    /// 首次读取初始化配置源订阅；与 TS `#ensureStarted` 一致只执行一次。
    fn ensure_started(&self) -> Result<(), String> {
        {
            let mut inner = self.lock();
            if inner.disposed {
                return Err(DISPOSED.to_owned());
            }
            if inner.started {
                return Ok(());
            }
            inner.started = true;
        }
        let weak = self.weak.clone();
        let unsubscribe = self.config_source.subscribe(Arc::new(move |reason: &str| {
            let Some(service) = weak.upgrade() else {
                return;
            };
            let suffix = if reason.is_empty() { "changed" } else { reason };
            let refresh_reason = format!("config:{suffix}");
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    // Source 驱动的后台失败通过 onDidRefreshError 报告，保留 LKG。
                    let _ = service.refresh(refresh_reason).await;
                });
            }
        }));
        self.lock().unsubscribe = Some(unsubscribe);
        Ok(())
    }

    /// 缓存读取；无快照时共享当前 in-flight 轮次，否则发起 `start` 轮。
    pub async fn read(self: &Arc<Self>) -> Result<Arc<Snapshot>, String> {
        self.assert_not_disposed()?;
        self.ensure_started()?;
        if let Some(snapshot) = self.last_snapshot() {
            return Ok(snapshot);
        }
        let in_flight = self
            .lock()
            .drive_result
            .as_ref()
            .map(watch::Sender::subscribe);
        if let Some(mut receiver) = in_flight {
            loop {
                if let Some(outcome) = receiver.borrow().clone() {
                    return outcome;
                }
                if receiver.changed().await.is_err() {
                    return Err(DISPOSED.to_owned());
                }
            }
        }
        self.refresh("start").await
    }

    /// 请求一轮覆盖本次 generation 的刷新。
    pub async fn refresh(
        self: &Arc<Self>,
        reason: impl Into<String>,
    ) -> Result<Arc<Snapshot>, String> {
        self.assert_not_disposed()?;
        self.ensure_started()?;
        let receiver = {
            let mut inner = self.lock();
            if inner.disposed {
                return Err(DISPOSED.to_owned());
            }
            inner.requested_generation += 1;
            let generation = inner.requested_generation;
            inner.push_reason(&reason.into());
            let (sender, receiver) = oneshot::channel();
            inner.waiters.push(Waiter { generation, sender });
            receiver
        };
        self.ensure_driver();
        receiver.await.map_err(|_| DISPOSED.to_owned())?
    }

    pub fn on_did_change(&self, listener: ChangeListener) -> Unsubscribe {
        let listeners = Arc::clone(&self.change_listeners);
        listeners
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(Arc::clone(&listener));
        Box::new(move || remove_listener(&listeners, &listener))
    }

    pub fn on_did_refresh_error(&self, listener: ErrorListener) -> Unsubscribe {
        let listeners = Arc::clone(&self.error_listeners);
        listeners
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(Arc::clone(&listener));
        Box::new(move || remove_listener(&listeners, &listener))
    }

    /// 与 TS `dispose` 一致：拒绝等待中的请求、退订配置源、清空监听器。
    pub fn dispose(&self) {
        let (waiters, unsubscribe) = {
            let mut inner = self.lock();
            if inner.disposed {
                return;
            }
            inner.disposed = true;
            (inner.take_waiters(u64::MAX), inner.unsubscribe.take())
        };
        if let Some(unsubscribe) = unsubscribe {
            unsubscribe();
        }
        for waiter in waiters {
            let _ = waiter.send(Err(DISPOSED.to_owned()));
        }
        self.change_listeners
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clear();
        self.error_listeners
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clear();
    }

    fn ensure_driver(self: &Arc<Self>) {
        let sender = {
            let mut inner = self.lock();
            if inner.driver_active || inner.disposed || inner.pending_reasons.is_empty() {
                return;
            }
            inner.driver_active = true;
            let (sender, _receiver) = watch::channel(None);
            inner.drive_result = Some(sender.clone());
            sender
        };
        let service = Arc::clone(self);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move { service.drive(sender).await });
        }
    }

    async fn drive(self: Arc<Self>, result_sender: watch::Sender<Option<RoundOutcome>>) {
        let outcome = self.run_loop().await;
        let _ = result_sender.send(Some(outcome));
        let restart = {
            let mut inner = self.lock();
            inner.driver_active = false;
            inner.drive_result = None;
            !inner.disposed && !inner.pending_reasons.is_empty()
        };
        if restart {
            self.ensure_driver();
        }
    }

    async fn run_loop(self: &Arc<Self>) -> RoundOutcome {
        let mut latest = self.last_snapshot();
        loop {
            let (generation, reasons) = {
                let mut inner = self.lock();
                if inner.disposed {
                    return Err(DISPOSED.to_owned());
                }
                if inner.pending_reasons.is_empty() {
                    break;
                }
                let generation = inner.requested_generation;
                let reasons = std::mem::take(&mut inner.pending_reasons);
                (generation, reasons)
            };
            self.run_round(&mut latest, generation, reasons).await?;
        }
        latest.ok_or_else(|| NOT_STARTED.to_owned())
    }

    async fn run_round(
        self: &Arc<Self>,
        latest: &mut Option<Arc<Snapshot>>,
        generation: u64,
        reasons: Vec<String>,
    ) -> Result<(), String> {
        let config = match self.config_source.read().await {
            Ok(config) => config,
            Err(error) => return self.handle_failure(latest, None, generation, &reasons, error),
        };
        let (previous_providers, previous_states) = match latest {
            Some(snapshot) => (snapshot.providers.clone(), snapshot.states.clone()),
            None => (ProviderConfigMap::empty(), None),
        };
        let resolved = self
            .resolver
            .resolve(ResolveRequest {
                config_revision: config.mode_builtin_revision.clone(),
                configured_providers: config.mode_builtin_providers.clone(),
                previous_providers,
                previous_states,
                reasons: reasons.clone(),
            })
            .await;
        if self.lock().disposed {
            return self.handle_failure(
                latest,
                Some(&config),
                generation,
                &reasons,
                DISPOSED.to_owned(),
            );
        }
        let current = match self.config_source.read().await {
            Ok(current) => current,
            Err(error) => {
                return self.handle_failure(latest, Some(&config), generation, &reasons, error)
            }
        };
        if self.lock().disposed {
            return self.handle_failure(
                latest,
                Some(&config),
                generation,
                &reasons,
                DISPOSED.to_owned(),
            );
        }
        let superseded = {
            let mut inner = self.lock();
            if !inner.pending_reasons.is_empty()
                || current.mode_builtin_revision != config.mode_builtin_revision
            {
                inner.push_reason("superseded-resolution");
                true
            } else {
                false
            }
        };
        if superseded {
            return Ok(());
        }
        let output = match resolved {
            Ok(output) => output,
            Err(error) => {
                return self.handle_failure(latest, Some(&config), generation, &reasons, error)
            }
        };
        let snapshot = create_account_provider_config_snapshot(
            &config.mode_builtin_revision,
            output.providers,
            Some(output.states),
        );
        let published = self.publish(generation, snapshot, &reasons);
        *latest = Some(published);
        Ok(())
    }

    /// 发布快照：revision 未变时只回应等待者；否则更新快照并在已有旧快照时通知变更。
    fn publish(&self, generation: u64, snapshot: Snapshot, reasons: &[String]) -> Arc<Snapshot> {
        let snapshot = Arc::new(snapshot);
        let (senders, change_reason, published) = {
            let mut inner = self.lock();
            let previous = inner.snapshot.clone();
            if let Some(previous) = previous
                .as_ref()
                .filter(|previous| previous.revision == snapshot.revision)
            {
                (inner.take_waiters(generation), None, Arc::clone(previous))
            } else {
                let had_snapshot = previous.is_some();
                inner.snapshot = Some(Arc::clone(&snapshot));
                (
                    inner.take_waiters(generation),
                    had_snapshot.then(|| reasons.join(",")),
                    Arc::clone(&snapshot),
                )
            }
        };
        for sender in senders {
            let _ = sender.send(Ok(Arc::clone(&published)));
        }
        if let Some(reason) = change_reason {
            let listeners: Vec<_> = self
                .change_listeners
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clone();
            for listener in listeners {
                listener(&reason);
            }
        }
        published
    }

    /// 失败处理：先广播错误；首次失败且已读到配置时发布 fail-closed 并继续，
    /// 否则回绝本 generation 的等待者并终止本轮。
    fn handle_failure(
        &self,
        latest: &mut Option<Arc<Snapshot>>,
        config: Option<&ProviderConfigSnapshot>,
        generation: u64,
        reasons: &[String],
        error: String,
    ) -> Result<(), String> {
        let event = RefreshErrorEvent {
            error: error.clone(),
            reasons: reasons.to_vec(),
        };
        let listeners: Vec<_> = self
            .error_listeners
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        for listener in listeners {
            listener(&event);
        }
        if latest.is_none() {
            if let Some(config) = config {
                let snapshot = Arc::new(create_fail_closed_account_provider_config_snapshot(
                    &config.mode_builtin_revision,
                    &config.mode_builtin_providers,
                ));
                {
                    let mut inner = self.lock();
                    inner.snapshot = Some(Arc::clone(&snapshot));
                }
                for sender in self.lock().take_waiters(generation) {
                    let _ = sender.send(Ok(Arc::clone(&snapshot)));
                }
                *latest = Some(snapshot);
                return Ok(());
            }
        }
        for sender in self.lock().take_waiters(generation) {
            let _ = sender.send(Err(error.clone()));
        }
        Err(error)
    }
}

fn remove_listener<T: ?Sized>(listeners: &Mutex<Vec<Arc<T>>>, target: &Arc<T>) {
    let mut listeners = listeners
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    if let Some(index) = listeners
        .iter()
        .position(|listener| Arc::ptr_eq(listener, target))
    {
        listeners.remove(index);
    }
}

/// 生产配置源：按路径读取 Bundled/Personal/Active 组合快照。
pub struct SnapshotConfigSource {
    bundled_path: String,
    active_path: Option<String>,
    personal_path: std::path::PathBuf,
    listeners: Arc<Mutex<Vec<ChangeListener>>>,
}

impl SnapshotConfigSource {
    pub fn new(
        bundled_path: impl Into<String>,
        active_path: Option<String>,
        personal_path: impl Into<std::path::PathBuf>,
    ) -> Self {
        Self {
            bundled_path: bundled_path.into(),
            active_path,
            personal_path: personal_path.into(),
            listeners: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 配置仓库轮询/写入成功后由宿主调用，驱动账号层刷新。
    pub fn notify(&self, reason: &str) {
        let listeners: Vec<_> = self
            .listeners
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        for listener in listeners {
            listener(reason);
        }
    }
}

impl ConfigSourcePort for SnapshotConfigSource {
    fn read(&self) -> ServiceFuture<'_, Result<ProviderConfigSnapshot, String>> {
        Box::pin(async move {
            read_snapshot(
                &self.bundled_path,
                &self.personal_path,
                self.active_path.as_deref(),
            )
            .map_err(|error| error.to_string())
        })
    }

    fn subscribe(&self, listener: ChangeListener) -> Unsubscribe {
        let listeners = Arc::clone(&self.listeners);
        listeners
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(Arc::clone(&listener));
        Box::new(move || remove_listener(&listeners, &listener))
    }
}

/// 生产 Resolver：串行复用 `AccountProviderConfigSource` 的 Overlay 解析。
pub struct OverlayResolver<O: AccountProviderOverlayOptions> {
    source: tokio::sync::Mutex<AccountProviderConfigSource<O>>,
}

impl<O: AccountProviderOverlayOptions + Send + Sync> OverlayResolver<O> {
    pub fn new(options: O) -> Self {
        Self {
            source: tokio::sync::Mutex::new(AccountProviderConfigSource::new(options)),
        }
    }
}

impl<O: AccountProviderOverlayOptions + Send + Sync> ResolverPort for OverlayResolver<O> {
    fn resolve(
        &self,
        request: ResolveRequest,
    ) -> ServiceFuture<'_, Result<AccountProviderResolveOutput, String>> {
        Box::pin(async move {
            let mut source = self.source.lock().await;
            let snapshot = source
                .resolve_snapshot(AccountProviderResolveInput {
                    config_revision: &request.config_revision,
                    configured_providers: &request.configured_providers,
                    previous_providers: &request.previous_providers,
                    previous_states: request.previous_states.as_ref(),
                    reasons: &request.reasons,
                })
                .await
                .map_err(|error| error.to_string())?;
            Ok(AccountProviderResolveOutput {
                providers: snapshot.providers,
                states: snapshot.states.unwrap_or_default(),
            })
        })
    }
}
