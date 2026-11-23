//! scoped 服务面登记表（对应 TS `createScopedAttachmentRegistry`）。
//!
//! 真源：`packages/desktop/src/host-standalone/scopedAttachments.ts`。
//! 规范见 `docs/specs/scoped-server-registry.md`。
//!
//! 一次 remote workspace 挂接需要**独立的服务面**：renderer 那一侧用
//! `[0xFF][scopeId][payload]` 前缀区分，Host 这侧就要为每个 scope 起一个独立
//! `ChannelServer`，并把它的出站帧加同样的前缀。
//!
//! 三条时序硬要求（每条都对应一个真实故障）：
//!
//! 1. **`attach` 不 ready**：页面端口还没注册时发 `Initialize` 会被解复用按未知
//!    scope 丢弃且不重发，客户端永久停在 Uninitialized。所以先建、后 `start`。
//! 2. **`register` 失败要回收 scope**：否则 scope id 被占着，`count()` 也对不上。
//! 3. **`detach` 必须先停转发再 dispose**：反过来会让 dispose 期间的帧写进一个
//!    已经没人读的通道，或让 detach 之后仍有帧漏出去。

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rpc_wire::scope::{decode_frame, encode_scoped_frame, DecodedFrame};
use tokio::sync::mpsc;

use crate::{ChannelServer, Subscription};

/// scope 出站帧的去处（页面桥那一侧）。入参是**已经加好 scope 前缀**的完整帧。
pub type ScopedFrameSink = Arc<dyn Fn(Vec<u8>) + Send + Sync>;

/// 壳侧结构化日志（`None` 时静默）。
pub type ScopeLogSink = Arc<dyn Fn(&str) + Send + Sync>;

type DetachListener = Arc<dyn Fn(u32) + Send + Sync>;

struct ScopeEntry {
    server: ChannelServer,
    forwarder: tokio::task::JoinHandle<()>,
}

#[derive(Default)]
struct Inner {
    next_scope_id: u32,
    scopes: BTreeMap<u32, ScopeEntry>,
    detach_listeners: HashMap<String, DetachListener>,
}

/// 一次挂接的句柄（TS `ScopedAttachment`）。
pub struct ScopedAttachment {
    scope_id: u32,
    /// 页面端口就绪：发 `Initialize`。
    start: Box<dyn Fn() + Send + Sync>,
    /// 回收 scope（与 `detach(scope_id)` 等价）。
    dispose: Box<dyn Fn() + Send + Sync>,
}

impl ScopedAttachment {
    pub fn scope_id(&self) -> u32 {
        self.scope_id
    }

    /// 页面桥注册完 `ChannelClient` 后调用：现在才发 `Initialize`。
    pub fn start(&self) {
        (self.start)();
    }

    pub fn dispose(&self) {
        (self.dispose)();
    }
}

pub struct ScopedServerRegistry {
    inner: Arc<Mutex<Inner>>,
    out: ScopedFrameSink,
    on_log: Option<ScopeLogSink>,
    timeout: Duration,
}

impl ScopedServerRegistry {
    pub fn new(out: ScopedFrameSink) -> Self {
        Self::with_options(out, None, Duration::from_millis(1000))
    }

    pub fn with_options(
        out: ScopedFrameSink,
        on_log: Option<ScopeLogSink>,
        timeout: Duration,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::default())),
            out,
            on_log,
            timeout,
        }
    }

    fn log(&self, message: &str) {
        if let Some(on_log) = &self.on_log {
            on_log(message);
        }
    }

    /// 建 scope 并交给调用方注册频道。
    ///
    /// **不 ready**：调用方拿到句柄后必须先让页面端口就绪，再 `start()`。
    /// `register` 返回错误时本方法会把 scope 回收掉（不留下悬空 id）。
    pub fn attach<F, E>(&self, register: F) -> Result<ScopedAttachment, E>
    where
        F: FnOnce(&mut ChannelServer) -> Result<(), E>,
    {
        let scope_id = {
            let mut inner = self.inner.lock().expect("scope registry poisoned");
            inner.next_scope_id = inner
                .next_scope_id
                .checked_add(1)
                .expect("scope id 耗尽（u32 上限）");
            inner.next_scope_id
        };

        // 每个 scope 一条独立出站通道：ChannelServer 写进来，转发器加前缀后送出。
        let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let out = Arc::clone(&self.out);
        let forwarder = tokio::spawn(async move {
            while let Some(frame) = rx.recv().await {
                match encode_scoped_frame(scope_id, &frame) {
                    Ok(wrapped) => out(wrapped),
                    // scope id 恒 >= 1，编码失败只可能是 bug；不静默吞掉。
                    Err(error) => eprintln!("scoped frame encode failed: {error:?}"),
                }
            }
        });

        // defer_init = true：等 start() 再发 Initialize。
        let mut server = ChannelServer::new(
            tx,
            format!("mode-host/scope-{scope_id}"),
            self.timeout,
            true,
        );
        if let Err(error) = register(&mut server) {
            // 注册失败必须回收：否则 scope id 被占着、count() 也对不上。
            forwarder.abort();
            self.log(&format!("scope_register_failed scope={scope_id}"));
            return Err(error);
        }

        {
            let mut inner = self.inner.lock().expect("scope registry poisoned");
            inner
                .scopes
                .insert(scope_id, ScopeEntry { server, forwarder });
        }
        self.log(&format!("scope_attached scope={scope_id}"));

        let registry = self.clone_handle();
        let start_registry = self.clone_handle();
        Ok(ScopedAttachment {
            scope_id,
            start: Box::new(move || {
                start_registry.start(scope_id);
            }),
            dispose: Box::new(move || {
                registry.detach(scope_id);
            }),
        })
    }

    fn clone_handle(&self) -> ScopedServerRegistry {
        ScopedServerRegistry {
            inner: Arc::clone(&self.inner),
            out: Arc::clone(&self.out),
            on_log: self.on_log.clone(),
            timeout: self.timeout,
        }
    }

    /// 页面端口就绪：发 `Initialize`。未知 scope 返回 `false`。
    pub fn start(&self, scope_id: u32) -> bool {
        let mut inner = self.inner.lock().expect("scope registry poisoned");
        let Some(entry) = inner.scopes.get_mut(&scope_id) else {
            drop(inner);
            self.log(&format!("scope_ready_unknown scope={scope_id}"));
            return false;
        };
        entry.server.ready();
        drop(inner);
        self.log(&format!("scope_ready scope={scope_id}"));
        true
    }

    /// 回收 scope。未知 scope 返回 `false`。
    ///
    /// 顺序：先从表里摘掉 → 停转发 → dispose 服务端 → 通知监听者。
    /// 反过来（先 dispose 再停转发）会让 dispose 期间的帧写进没人读的通道。
    pub fn detach(&self, scope_id: u32) -> bool {
        let entry = {
            let mut inner = self.inner.lock().expect("scope registry poisoned");
            inner.scopes.remove(&scope_id)
        };
        let Some(mut entry) = entry else {
            return false;
        };
        entry.forwarder.abort();
        entry.server.dispose();
        self.log(&format!("scope_detached scope={scope_id}"));
        let listeners: Vec<DetachListener> = {
            let inner = self.inner.lock().expect("scope registry poisoned");
            inner.detach_listeners.values().cloned().collect()
        };
        for listener in listeners {
            listener(scope_id);
        }
        true
    }

    /// 把一个**已带 scope 前缀**的帧交给对应 scope 的服务面。
    ///
    /// 返回 `false` = 不是已知 scope（调用方按未知帧丢弃，与 TS 解复用同）。
    pub fn handle_frame(&self, frame: &[u8]) -> bool {
        let Ok(DecodedFrame::Scoped { scope_id, payload }) = decode_frame(frame) else {
            return false;
        };
        let mut inner = self.inner.lock().expect("scope registry poisoned");
        let Some(entry) = inner.scopes.get_mut(&scope_id) else {
            return false;
        };
        entry.server.handle_frame(payload);
        true
    }

    /// scope 被回收时回调（业务层据此同步清理 attachment 表）。
    pub fn subscribe_detach(&self, listener: impl Fn(u32) + Send + Sync + 'static) -> Subscription {
        let id = uuid_like();
        self.inner
            .lock()
            .expect("scope registry poisoned")
            .detach_listeners
            .insert(id.clone(), Arc::new(listener));
        let inner = Arc::clone(&self.inner);
        Box::new(move || {
            inner
                .lock()
                .expect("scope registry poisoned")
                .detach_listeners
                .remove(&id);
        })
    }

    /// 回收全部 scope（Host 退出时）。
    pub fn dispose_all(&self) {
        let scope_ids: Vec<u32> = self
            .inner
            .lock()
            .expect("scope registry poisoned")
            .scopes
            .keys()
            .copied()
            .collect();
        for scope_id in scope_ids {
            self.detach(scope_id);
        }
        self.inner
            .lock()
            .expect("scope registry poisoned")
            .detach_listeners
            .clear();
    }

    /// 当前存活 scope 数。
    pub fn count(&self) -> usize {
        self.inner
            .lock()
            .expect("scope registry poisoned")
            .scopes
            .len()
    }

    /// 当前已分配的 scope id（诊断用；`count()` 才是业务判据）。
    pub fn active_scope_ids(&self) -> Vec<u32> {
        self.inner
            .lock()
            .expect("scope registry poisoned")
            .scopes
            .keys()
            .copied()
            .collect()
    }
}

/// 轻量唯一 id（只在本进程内做监听者键，不参与协议）。
fn uuid_like() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!("listener-{}", COUNTER.fetch_add(1, Ordering::SeqCst))
}
