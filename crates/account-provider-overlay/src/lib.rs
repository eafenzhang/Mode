//! 账号层 Provider Config Overlay 的 Rust 移植。
//!
//! 范围：连接解析、第三层 Provider Config 投影、Account State 与 snapshot revision。
//! 不做真实凭据/OAuth/网络、刷新编排、registry 非账号部分。规范见
//! `docs/specs/account-provider-overlay.md`。

mod config;
mod connection;
mod snapshot;
mod types;

pub use config::{
    build_account_provider_states, create_account_provider_config_resolver,
    resolve_account_provider_configs, AccountProviderConfigResolver, AccountProviderResolveResult,
    ResolveAccountProviderConfigsInput,
};
pub use connection::{AccountProviderConnectionResolver, AccountProviderOverlayOptions, BoxFuture};
pub use snapshot::{
    create_account_provider_config_snapshot, create_fail_closed_account_provider_config_snapshot,
    AccountProviderConfigSnapshot, AccountProviderConfigSource,
};
pub use types::{
    AccountProviderConnectionResult, AccountProviderConnectionSettings,
    AccountProviderConnectionStatus, AccountProviderFamilyAvailabilityInput,
    AccountProviderOverlayError, AccountProviderResolveInput, AccountProviderResolveOutput,
    CodingPlanAvailabilityProvider, CodingPlanAvailabilityResult, CodingPlanUnavailableReason,
    PlanKind, ProviderFamilyConnectionSelection, ProviderFamilyConnectionSelectionSettings,
    ProviderFamilyDomain,
};

// 账号状态类型由 provider-config 提供，这里统一转出，调用方不必直接依赖底层 crate。
pub use provider_config::{
    AccountProviderAvailability, AccountProviderState, AccountProviderStates,
    AccountProviderUnavailableReason, ProviderConfig, ProviderConfigMap, ProviderConfigRule,
};
