//! 账号 Provider 事实链。
//!
//! 把 `provider-config-source`（Built-in + Personal 文件层）与 `account-provider-overlay`
//! （账号事实投影）串成 `agent-process::ProviderSource`，对应 TS
//! `AccountProviderService` + `createAccountProviderConfigSource` 的编排。
//!
//! 规范见 `docs/specs/provider-source-chain.md`；本模块不做网络/OAuth/凭据实现，
//! 这些事实由注入的 `AccountProviderOverlayOptions` 提供。

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use account_provider_overlay::{
    create_fail_closed_account_provider_config_snapshot, AccountProviderConfigSnapshot,
    AccountProviderConfigSource, AccountProviderOverlayError, AccountProviderOverlayOptions,
    AccountProviderResolveInput,
};
use agent_process::{ProviderReadFuture, ProviderSource};
use provider_config::ProviderConfigMap;
use provider_config_source::{read_snapshot, SourceError};
use serde_json::Value;
use tokio::sync::Mutex;

/// 链路错误。配置层与账号层分别保留原始错误，便于调用方区分可重试性。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainError {
    Config(SourceError),
    Overlay(AccountProviderOverlayError),
}

impl fmt::Display for ChainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChainError::Config(error) => write!(formatter, "Provider 配置读取失败: {error}"),
            ChainError::Overlay(error) => write!(formatter, "账号 Provider 解析失败: {error}"),
        }
    }
}

impl std::error::Error for ChainError {}

/// 文件层 → 账号层 → 交付快照的编排，持有 last-known-good。
///
/// 一个实例对应一份配置根；`resolve` 通过 `&mut self` 串行，调用方负责并发封装
/// （见 [`AgentProviderSource`]）。
pub struct ProviderChain<O: AccountProviderOverlayOptions> {
    bundled_path: String,
    active_path: Option<String>,
    personal_path: PathBuf,
    overlay: AccountProviderConfigSource<O>,
    last: Option<AccountProviderConfigSnapshot>,
}

impl<O: AccountProviderOverlayOptions + Send + Sync> ProviderChain<O> {
    pub fn new(
        bundled_path: impl Into<String>,
        active_path: Option<String>,
        personal_path: impl Into<PathBuf>,
        options: O,
    ) -> Self {
        Self {
            bundled_path: bundled_path.into(),
            active_path,
            personal_path: personal_path.into(),
            overlay: AccountProviderConfigSource::new(options),
            last: None,
        }
    }

    /// 上一次成功（或 fail-closed）发布的快照。
    pub fn last_snapshot(&self) -> Option<&AccountProviderConfigSnapshot> {
        self.last.as_ref()
    }

    /// 读取文件层并执行账号解析。
    ///
    /// 与 TS `AccountProviderService.#runRefreshLoop` 的失败语义一致：
    /// - 首次解析失败 → 用 Built-in 生成 fail-closed 快照并发布；
    /// - 非首次失败 → 保留 last-known-good 并返回错误。
    pub async fn resolve(
        &mut self,
        reasons: &[String],
    ) -> Result<AccountProviderConfigSnapshot, ChainError> {
        let config = read_snapshot(
            &self.bundled_path,
            &self.personal_path,
            self.active_path.as_deref(),
        )
        .map_err(ChainError::Config)?;

        // 先取出上一份快照，避免同时借用 `self.last` 与 `self.overlay`。
        let previous = self.last.take();
        let empty_providers = ProviderConfigMap::empty();
        let previous_providers = previous
            .as_ref()
            .map(|snapshot| &snapshot.providers)
            .unwrap_or(&empty_providers);
        let previous_states = previous
            .as_ref()
            .and_then(|snapshot| snapshot.states.as_ref());

        let resolved = self
            .overlay
            .resolve_snapshot(AccountProviderResolveInput {
                // 账号解析只以 Built-in 规则为“已配置 Provider”，与 TS 一致：
                // Personal 层可以覆盖展示与连接，但不扩张账号族。
                config_revision: &config.mode_builtin_revision,
                configured_providers: &config.mode_builtin_providers,
                previous_providers,
                previous_states,
                reasons,
            })
            .await;

        match resolved {
            Ok(snapshot) => {
                self.last = Some(snapshot.clone());
                Ok(snapshot)
            }
            Err(error) => {
                if previous.is_none() {
                    let fail_closed = create_fail_closed_account_provider_config_snapshot(
                        &config.mode_builtin_revision,
                        &config.mode_builtin_providers,
                    );
                    self.last = Some(fail_closed.clone());
                    Ok(fail_closed)
                } else {
                    self.last = previous;
                    Err(ChainError::Overlay(error))
                }
            }
        }
    }
}

/// `ProviderSource` 适配器：把 [`ProviderChain`] 包装为 agent-process 可消费的异步源。
pub struct AgentProviderSource<O: AccountProviderOverlayOptions> {
    chain: Mutex<ProviderChain<O>>,
    reasons: Vec<String>,
}

impl<O: AccountProviderOverlayOptions + Send + Sync> AgentProviderSource<O> {
    pub fn new(
        bundled_path: impl Into<String>,
        active_path: Option<String>,
        personal_path: impl Into<PathBuf>,
        options: O,
    ) -> Self {
        Self {
            chain: Mutex::new(ProviderChain::new(
                bundled_path,
                active_path,
                personal_path,
                options,
            )),
            reasons: vec!["start".to_owned()],
        }
    }

    /// 覆盖传给账号层的刷新原因（默认 `["start"]`）。
    pub fn with_reasons(mut self, reasons: Vec<String>) -> Self {
        self.reasons = reasons;
        self
    }
}

impl<O> ProviderSource for AgentProviderSource<O>
where
    O: AccountProviderOverlayOptions + Send + Sync,
{
    fn read(&self) -> ProviderReadFuture<'_> {
        Box::pin(async move {
            let mut chain = self.chain.lock().await;
            let snapshot = chain
                .resolve(&self.reasons)
                .await
                .map_err(|error| error.to_string())?;
            Ok(to_agent_snapshot(&snapshot))
        })
    }
}

/// 把 Overlay 快照转成交付快照。
///
/// 线格式与 TS `modeAgentService` 一致：`providers` 是 `providerId -> config.toJSON()`
/// 的对象（不是 rule 数组），`states` 恒为对象。
pub fn to_agent_snapshot(
    snapshot: &AccountProviderConfigSnapshot,
) -> agent_process::AccountProviderConfigSnapshot {
    let providers: BTreeMap<String, Value> = snapshot
        .providers
        .entries()
        .map(|(provider_id, rule)| (provider_id.clone(), to_serde_json(&rule.config.to_json())))
        .collect();
    let states: BTreeMap<String, Value> = match &snapshot.states {
        Some(states) => serde_json::to_value(states)
            .ok()
            .and_then(|value| match value {
                Value::Object(map) => Some(map.into_iter().collect()),
                _ => None,
            })
            .unwrap_or_default(),
        None => BTreeMap::new(),
    };
    agent_process::AccountProviderConfigSnapshot {
        revision: snapshot.revision.clone(),
        based_on_mode_builtin_revision: snapshot.based_on_mode_builtin_revision.clone(),
        providers,
        states,
    }
}

fn to_serde_json(value: &provider_config::JsonValue) -> Value {
    serde_json::from_str(&value.to_json_string())
        .expect("JsonValue 序列化输出必为合法 JSON")
}
