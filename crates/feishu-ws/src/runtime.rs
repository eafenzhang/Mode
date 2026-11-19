//! 重连循环（spec §5.4；SDK `reConnect` :85393-85481 的对齐）。
//!
//! 每轮 = 注册（pull）→ 建连 → 会话；建连失败或注册失败按 `ReconnectCount`
//! （默认 -1 无限）与 `ReconnectInterval` 重试；断线（会话正常结束）后先等
//! `ReconnectNonce × random()` 抖动再进入重试。pong 更新的会话参数在重连时生效。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;

use crate::gateway::{FeishuConfig, PullConfig, SessionParams};
use crate::session::{run_session, SessionCallbacks, SessionOutcome, SessionTiming};

#[derive(Debug, Clone, Default)]
pub struct RuntimeTiming {
    pub session: SessionTiming,
    /// 注册响应缺省参数（同 WSConfig 默认）。
    pub defaults: SessionParams,
}

/// 主循环：直到 `shutdown` 置真或重试上限耗尽才返回（count = -1 时永不放弃）。
pub async fn run_connection(
    pull: Arc<dyn PullConfig>,
    config: FeishuConfig,
    callbacks: SessionCallbacks,
    timing: RuntimeTiming,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut failures: i64 = 0;
    // 当前生效的重连参数：注册/pong 都可能改写（SDK 的 wsConfig 语义）。
    let mut params = timing.defaults.clone();
    loop {
        if *shutdown.borrow_and_update() {
            return;
        }
        let outcome = attempt(
            &pull,
            &config,
            &callbacks,
            &timing,
            &mut params,
            &mut shutdown,
        )
        .await;
        if *shutdown.borrow_and_update() {
            return;
        }
        match outcome {
            SessionOutcome::Established => {
                // 曾经连上过：计数清零；断线后先 nonce 抖动再进入重试。
                failures = 0;
                let nonce = jittered_nonce(&params.reconnect_nonce);
                if !sleep_or_shutdown(&mut shutdown, nonce).await {
                    return;
                }
            }
            SessionOutcome::ConnectFailed => {
                failures += 1;
                if params.reconnect_count >= 0 && failures > params.reconnect_count {
                    crate::session::log_line(&format!(
                        "feishu reconnect gave up after {failures} attempt(s)"
                    ));
                    return;
                }
                // 首次失败先等 nonce 抖动，之后按 reconnectInterval 重试。
                let wait = if failures == 1 {
                    jittered_nonce(&params.reconnect_nonce)
                } else {
                    params.reconnect_interval
                };
                if !sleep_or_shutdown(&mut shutdown, wait).await {
                    return;
                }
            }
        }
    }
}

async fn attempt(
    pull: &Arc<dyn PullConfig>,
    config: &FeishuConfig,
    callbacks: &SessionCallbacks,
    timing: &RuntimeTiming,
    current: &mut SessionParams,
    shutdown: &mut watch::Receiver<bool>,
) -> SessionOutcome {
    let registered = match pull.pull(config).await {
        Ok(mut registered) => {
            // 注册响应缺省项回退 WSConfig 默认（spec §5.1 第 5 条）。
            if registered.ping_interval.is_zero() {
                registered.ping_interval = timing.defaults.ping_interval;
            }
            if registered.reconnect_interval.is_zero() {
                registered.reconnect_interval = timing.defaults.reconnect_interval;
            }
            if registered.reconnect_nonce.is_zero() {
                registered.reconnect_nonce = timing.defaults.reconnect_nonce;
            }
            registered
        }
        Err(error) => {
            crate::session::log_line(&format!("feishu WS registration failed: {error}"));
            return SessionOutcome::ConnectFailed;
        }
    };
    *current = registered.clone();
    let shared = Arc::new(Mutex::new(registered));
    let outcome = run_session(
        Arc::clone(&shared),
        callbacks.clone(),
        &timing.session,
        shutdown.clone(),
    )
    .await;
    // 会话期间 pong 可能改写了参数（ping/重连），把最新值交还重连循环。
    *current = shared
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_else(|poisoned| poisoned.into_inner().clone());
    outcome
}

fn jittered_nonce(nonce: &Duration) -> Duration {
    if nonce.is_zero() {
        return Duration::ZERO;
    }
    // 无 rand 依赖：uuid v4 前 8 字节归一为 [0,1) 抖动系数（SDK: nonce * Math.random()）。
    let bytes = uuid::Uuid::new_v4().into_bytes();
    let fraction = u64::from_be_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]) as f64
        / u64::MAX as f64;
    nonce.mul_f64(fraction)
}

async fn sleep_or_shutdown(shutdown: &mut watch::Receiver<bool>, duration: Duration) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(duration) => true,
        changed = shutdown.changed() => {
            if changed.is_err() {
                return false;
            }
            !(*shutdown.borrow_and_update())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_stays_within_nonce() {
        let nonce = Duration::from_millis(1000);
        for _ in 0..32 {
            let jittered = jittered_nonce(&nonce);
            assert!(jittered <= nonce);
        }
        assert_eq!(jittered_nonce(&Duration::ZERO), Duration::ZERO);
    }
}
