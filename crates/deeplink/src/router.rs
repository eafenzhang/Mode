//! 深链投递：冷启动缓存 + renderer 就绪后回放。
//!
//! ## 为什么需要这层
//!
//! 冷启动时操作系统会在**窗口还没起来**的时候就把 `mode://` 链接交进来
//! （Windows 走 argv、macOS 走 `open-url`）。此时渲染层还没有订阅者，直接投递就是丢事件。
//! 既有语义里解决这件事的是 `notifyRendererReady()` 这个握手：渲染层挂载完成、
//! 订阅装好之后通知宿主，宿主把缓存的深链补发出去
//! （见 `packages/shared/src/platform.ts` 的接口注释）。
//!
//! ## 为什么不是直接在 Tauri 上写
//!
//! 缓存/回放的时序是本模块的**被测对象**，而 `tauri::test` 需要一个 webview mock 运行时
//! （M1 骨架刻意不引入，见 `docs/plans/2026-10-15-tauri-skeleton.md` Task 2 的执行记录）。
//! 于是把「投递给谁」抽成 [`DeepLinkSink`]，路由逻辑本身不碰 Tauri，可以用一个记录用的
//! 替身在单测里把时序钉死；Tauri 侧只提供 [`DeepLinkSink`] 的一个实现。
use std::sync::Mutex;

use crate::intent::{self, DeepLinkIntent};

/// 深链的投递目标。Tauri 侧用事件实现，测试用记录器实现。
///
/// 除了类型化意图还要带上**原文**：`WorkspaceOpen` / `ShareImport` 变体只保了解析后的
/// 字段（`path` / `code`），而事件载荷与日志都需要整条链接。
pub trait DeepLinkSink {
    /// 投递一条意图。实现方不应在此阻塞。
    fn deliver(&self, raw_url: &str, intent: &DeepLinkIntent);
}

/// 深链路由：解析 + 就绪前缓存 + 就绪后回放。
///
/// 所有者声明：**本结构是「待投递深链」的唯一所有者**。渲染层的就绪状态由调用方通过
/// [`DeepLinkRouter::mark_renderer_ready`] 告知，本模块不自行判断渲染层状态。
pub struct DeepLinkRouter<S: DeepLinkSink> {
    sink: S,
    inner: Mutex<RouterState>,
}

#[derive(Default)]
struct RouterState {
    /// `notifyRendererReady` 之前收到的深链，按到达顺序排队（原文 + 解析结果）。
    pending: Vec<(String, DeepLinkIntent)>,
    renderer_ready: bool,
}

impl<S: DeepLinkSink> DeepLinkRouter<S> {
    pub fn new(sink: S) -> Self {
        Self {
            sink,
            inner: Mutex::new(RouterState::default()),
        }
    }

    /// 收到一条原始深链。
    ///
    /// 返回解析出的意图；`None` 表示这不是本应用认的深链（scheme 不认识）。
    /// 渲染层已就绪时立即投递，否则入队等 [`Self::mark_renderer_ready`]。
    ///
    /// [`DeepLinkIntent::Unrecognized`] 也算一条意图：照常排队与投递，让渲染层能观察到
    /// 未认领的链接，而不是静默消失。
    pub fn dispatch(&self, raw: &str) -> Option<DeepLinkIntent> {
        let parsed = intent::parse(raw)?;
        let mut state = self.lock();
        if state.renderer_ready {
            drop(state);
            self.sink.deliver(raw, &parsed);
        } else {
            state.pending.push((raw.to_string(), parsed.clone()));
        }
        Some(parsed)
    }

    /// 渲染层握手完成：回放积压的深链，此后到达的即时投递。
    ///
    /// **幂等**：重复调用不会重复回放（第二次调用时队列已空）。
    /// 返回本次回放的条数，便于调用方记日志。
    pub fn mark_renderer_ready(&self) -> usize {
        let flushed = {
            let mut state = self.lock();
            state.renderer_ready = true;
            std::mem::take(&mut state.pending)
        };
        for (raw, intent) in &flushed {
            self.sink.deliver(raw, intent);
        }
        flushed.len()
    }

    /// 当前积压条数（测试与可观测性用）。
    pub fn pending_len(&self) -> usize {
        self.lock().pending.len()
    }

    /// 临界区内不做 IO（投递都在锁外），因此中毒不会留下不一致状态，可直接取回内部值。
    fn lock(&self) -> std::sync::MutexGuard<'_, RouterState> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingSink {
        delivered: Mutex<Vec<(String, DeepLinkIntent)>>,
    }

    impl RecordingSink {
        fn take(&self) -> Vec<(String, DeepLinkIntent)> {
            std::mem::take(&mut *self.delivered.lock().unwrap())
        }
    }

    impl DeepLinkSink for std::sync::Arc<RecordingSink> {
        fn deliver(&self, raw_url: &str, intent: &DeepLinkIntent) {
            self.delivered
                .lock()
                .unwrap()
                .push((raw_url.to_string(), intent.clone()));
        }
    }

    fn router() -> (
        std::sync::Arc<RecordingSink>,
        DeepLinkRouter<std::sync::Arc<RecordingSink>>,
    ) {
        let sink = std::sync::Arc::new(RecordingSink::default());
        (sink.clone(), DeepLinkRouter::new(sink))
    }

    /// 断言一条投递：原文 + 意图成对。
    fn delivered(raw: &str, intent: DeepLinkIntent) -> (String, DeepLinkIntent) {
        (raw.to_string(), intent)
    }

    #[test]
    fn cold_start_links_wait_for_the_renderer_handshake() {
        let (sink, router) = router();
        assert!(router.dispatch("mode://oauth/callback?state=s1").is_some());
        assert_eq!(sink.take(), vec![], "渲染层就绪前不得投递");
        assert_eq!(router.pending_len(), 1);

        assert_eq!(router.mark_renderer_ready(), 1);
        assert_eq!(
            sink.take(),
            vec![delivered(
                "mode://oauth/callback?state=s1",
                DeepLinkIntent::OAuthCallback {
                    url: "mode://oauth/callback?state=s1".to_string()
                }
            )]
        );
    }

    #[test]
    fn warm_links_are_delivered_immediately_in_arrival_order() {
        let (sink, router) = router();
        router.mark_renderer_ready();
        router.dispatch("mode://payment/callback?state=p1");
        router.dispatch("mode://share/import?code=abc");
        assert_eq!(router.pending_len(), 0);
        assert_eq!(
            sink.take(),
            vec![
                delivered(
                    "mode://payment/callback?state=p1",
                    DeepLinkIntent::PaymentCallback {
                        url: "mode://payment/callback?state=p1".to_string()
                    }
                ),
                delivered(
                    "mode://share/import?code=abc",
                    DeepLinkIntent::ShareImport {
                        code: "abc".to_string()
                    }
                ),
            ]
        );
    }

    #[test]
    fn handshake_is_idempotent_and_does_not_replay_twice() {
        let (sink, router) = router();
        router.dispatch("mode://oauth/callback?state=s1");
        assert_eq!(router.mark_renderer_ready(), 1);
        assert_eq!(router.mark_renderer_ready(), 0, "第二次握手不应重放");
        assert_eq!(sink.take().len(), 1);
    }
    #[test]
    fn unknown_scheme_is_reported_as_unhandled_not_queued() {
        let (sink, router) = router();
        assert_eq!(router.dispatch("https://oauth/callback?state=x"), None);
        assert_eq!(router.pending_len(), 0);
        assert_eq!(sink.take(), vec![]);
    }

    #[test]
    fn unrecognized_route_is_still_delivered_so_it_can_be_observed() {
        let (sink, router) = router();
        // `mode://unknown/route` 是「认得出 scheme、没有规则」：必须能观察到，不能静默丢。
        assert_eq!(
            router.dispatch("mode://unknown/route?x=1"),
            Some(DeepLinkIntent::Unrecognized {
                url: "mode://unknown/route?x=1".to_string()
            })
        );
        router.mark_renderer_ready();
        assert_eq!(sink.take().len(), 1);
    }
}
