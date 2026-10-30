//! 连接结果 → 第三层 Provider Config Overlay 与 Account State。
//!
//! 真源：`packages/provider/src/account-provider-resolution.ts` 的
//! `resolveAccountProviderConfigs` 与 `createAccountProviderConfigResolver` 内联的
//! states 生成。

use std::collections::HashMap;

use indexmap::IndexMap;
use provider_config::{
    AccountProviderState, AccountProviderStates, Field, ProviderAccess, ProviderConfig,
    ProviderConfigMap, ProviderConfigRule, ZhipuAccountAccess, ZhipuAccountMode,
};

use crate::connection::AccountProviderConnectionResolver;
use crate::types::{
    AccountProviderConnectionResult, AccountProviderConnectionStatus, AccountProviderOverlayError,
    AccountProviderResolveInput, AccountProviderResolveOutput,
};

#[derive(Debug, Clone, Copy)]
pub struct ResolveAccountProviderConfigsInput<'a> {
    pub configured_providers: &'a ProviderConfigMap,
    pub previous_providers: &'a ProviderConfigMap,
    pub connections: &'a [AccountProviderConnectionResult],
}

/// 把账号连接结果转换为 Registry 使用的第三层 Account Provider Config。
pub fn resolve_account_provider_configs(
    input: ResolveAccountProviderConfigsInput<'_>,
) -> Result<ProviderConfigMap, AccountProviderOverlayError> {
    let connection_by_provider_id =
        index_connections(input.configured_providers, input.connections)?;
    let mut values: IndexMap<String, ProviderConfigRule> = IndexMap::new();

    for (provider_id, rule) in input.configured_providers.entries() {
        let Some(access) = account_access(&rule.config) else {
            continue;
        };
        let connection = connection_by_provider_id.get(provider_id.as_str()).copied();

        let config = match connection {
            Some(connection)
                if matches!(
                    connection.status,
                    AccountProviderConnectionStatus::Available
                        | AccountProviderConnectionStatus::Pending
                ) =>
            {
                let entitled = connection.status == AccountProviderConnectionStatus::Available;
                if access.mode.value() == Some(&ZhipuAccountMode::StartPlan) {
                    ProviderConfig {
                        // 明确空模型是本轮权威结果，不能保留已经失效的旧白名单。
                        access: zhipu_access(Some(entitled)),
                        builtin_model_ids: Field::Value(normalize_model_ids(
                            connection.models.as_deref(),
                        )),
                        ..ProviderConfig::default()
                    }
                } else {
                    ProviderConfig {
                        access: zhipu_access(Some(entitled)),
                        ..ProviderConfig::default()
                    }
                }
            }
            Some(connection)
                if connection.status == AccountProviderConnectionStatus::Unavailable =>
            {
                entitlement_overlay(false)
            }
            _ => {
                let reset_previous = connection.is_some_and(|connection| connection.reset_previous);
                if reset_previous {
                    entitlement_overlay(false)
                } else {
                    match input.previous_providers.get(provider_id) {
                        Some(previous) => previous.config.clone(),
                        None => entitlement_overlay(false),
                    }
                }
            }
        };

        values.insert(
            provider_id.clone(),
            ProviderConfigRule {
                provider_id: provider_id.clone(),
                template_id: Field::Absent,
                provider_name: Field::Absent,
                enabled: Field::Absent,
                config,
            },
        );
    }

    Ok(ProviderConfigMap { values })
}

/// states 生成规则（与 `createAccountProviderConfigResolver` 内联逻辑一致）。
pub fn build_account_provider_states(
    connections: &[AccountProviderConnectionResult],
    providers: &ProviderConfigMap,
    previous_states: Option<&AccountProviderStates>,
) -> AccountProviderStates {
    let mut states: AccountProviderStates = IndexMap::with_capacity(connections.len());
    for connection in connections {
        let previous = if connection.reset_previous {
            None
        } else {
            previous_states.and_then(|states| states.get(&connection.provider_id))
        };

        // unknown 仅保留上次展示事实；current 始终来自本轮选择，不能复活旧连接。
        // 原因字段与 availability 同规则：unknown 沿用上一轮。
        let unavailable_reason = match connection.status {
            AccountProviderConnectionStatus::Unknown => {
                previous.and_then(|previous| previous.unavailable_reason)
            }
            AccountProviderConnectionStatus::Unavailable => connection.unavailable_reason,
            _ => None,
        };
        let availability = match (connection.status, previous) {
            (AccountProviderConnectionStatus::Unknown, Some(previous)) => previous.availability,
            (status, _) => status.availability(),
        };
        let entitled = providers
            .get(&connection.provider_id)
            .is_some_and(|rule| access_entitled(&rule.config));
        let current = match (connection.status, previous) {
            (AccountProviderConnectionStatus::Unknown, Some(previous)) => {
                connection.current.or(previous.current)
            }
            _ => connection.current,
        };
        let effective_at = match (connection.status, previous) {
            (AccountProviderConnectionStatus::Unknown, Some(previous)) => {
                connection.effective_at.or(previous.effective_at)
            }
            _ => connection.effective_at,
        };

        states.insert(
            connection.provider_id.clone(),
            AccountProviderState {
                availability,
                entitled,
                unavailable_reason,
                current,
                // TS 字面量总是写入 connectionKey（可能为 undefined，随后被
                // JSON.stringify 丢弃），因此不继承 previous。
                connection_key: connection.connection_key.clone(),
                effective_at,
            },
        );
    }
    states
}

/// 组合 resolver：`createAccountProviderConfigResolver(connectionResolver)` 的 Rust 形态。
pub struct AccountProviderConfigResolver<O> {
    connection_resolver: AccountProviderConnectionResolver<O>,
}

/// 含连接结果的完整输出（对应 TS 对 connectionResolver 的 tap）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountProviderResolveResult {
    pub connections: Vec<AccountProviderConnectionResult>,
    pub providers: ProviderConfigMap,
    pub states: AccountProviderStates,
}

impl<O: crate::connection::AccountProviderOverlayOptions> AccountProviderConfigResolver<O> {
    pub fn new(options: O) -> Self {
        Self {
            connection_resolver: AccountProviderConnectionResolver::new(options),
        }
    }

    pub fn connection_resolver(&self) -> &AccountProviderConnectionResolver<O> {
        &self.connection_resolver
    }

    pub async fn resolve(
        &mut self,
        input: AccountProviderResolveInput<'_>,
    ) -> Result<AccountProviderResolveOutput, AccountProviderOverlayError> {
        let result = self.resolve_with_connections(input).await?;
        Ok(AccountProviderResolveOutput {
            providers: result.providers,
            states: result.states,
        })
    }

    pub async fn resolve_with_connections(
        &mut self,
        input: AccountProviderResolveInput<'_>,
    ) -> Result<AccountProviderResolveResult, AccountProviderOverlayError> {
        // TS 只把 configRevision/configuredProviders/reasons 传给连接解析器。
        let _ = input.config_revision;
        let connections = self
            .connection_resolver
            .resolve(input.configured_providers, input.reasons)
            .await?;
        let providers = resolve_account_provider_configs(ResolveAccountProviderConfigsInput {
            configured_providers: input.configured_providers,
            previous_providers: input.previous_providers,
            connections: &connections,
        })?;
        let states = build_account_provider_states(&connections, &providers, input.previous_states);
        Ok(AccountProviderResolveResult {
            connections,
            providers,
            states,
        })
    }
}

pub fn create_account_provider_config_resolver<
    O: crate::connection::AccountProviderOverlayOptions,
>(
    options: O,
) -> AccountProviderConfigResolver<O> {
    AccountProviderConfigResolver::new(options)
}

fn index_connections<'a>(
    configured_providers: &ProviderConfigMap,
    connections: &'a [AccountProviderConnectionResult],
) -> Result<HashMap<&'a str, &'a AccountProviderConnectionResult>, AccountProviderOverlayError> {
    let mut result = HashMap::with_capacity(connections.len());
    for connection in connections {
        if result.contains_key(connection.provider_id.as_str()) {
            return Err(AccountProviderOverlayError::new(format!(
                "重复 Account Provider 连接结果: {}",
                connection.provider_id
            )));
        }
        let Some(configured) = configured_providers.get(&connection.provider_id) else {
            return Err(AccountProviderOverlayError::new(format!(
                "Account 连接指向未配置 Provider: {}",
                connection.provider_id
            )));
        };
        if account_access(&configured.config).is_none() {
            return Err(AccountProviderOverlayError::new(format!(
                "Account 连接指向非 Account Provider: {}",
                connection.provider_id
            )));
        }
        result.insert(connection.provider_id.as_str(), connection);
    }
    Ok(result)
}

fn account_access(config: &ProviderConfig) -> Option<&ZhipuAccountAccess> {
    match &config.access {
        Field::Value(ProviderAccess::ZhipuAccount(access)) => Some(access),
        _ => None,
    }
}

fn access_entitled(config: &ProviderConfig) -> bool {
    account_access(config).is_some_and(|access| access.entitled.value() == Some(&true))
}

fn zhipu_access(entitled: Option<bool>) -> Field<ProviderAccess> {
    Field::Value(ProviderAccess::ZhipuAccount(ZhipuAccountAccess {
        account_type: Field::Absent,
        mode: Field::Absent,
        entitled: entitled.map_or(Field::Absent, Field::Value),
    }))
}

fn entitlement_overlay(entitled: bool) -> ProviderConfig {
    ProviderConfig {
        access: zhipu_access(Some(entitled)),
        ..ProviderConfig::default()
    }
}

fn normalize_model_ids(values: Option<&[String]>) -> Vec<String> {
    let values = values.unwrap_or(&[]);
    let mut result = Vec::with_capacity(values.len());
    for value in values {
        let model_id = value.trim();
        if model_id.is_empty() {
            continue;
        }
        result.push(model_id.to_owned());
    }
    result
}
