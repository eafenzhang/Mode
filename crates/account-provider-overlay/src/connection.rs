//! 账号连接解析。
//!
//! 真源：`packages/services/src/model-provider/accountProviderConnectionResolver.ts`。
//!
//! 一个 resolver 实例持有一条 `previousScopes` 链（对应 TS 闭包变量）：
//! 只有整轮成功发布后才整体替换，失败时不推进 last-known-good。

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use indexmap::IndexMap;
use provider_config::{
    AccountProviderUnavailableReason, Field, ProviderAccess, ProviderConfig, ProviderConfigMap,
    ZhipuAccountAccess, ZhipuAccountMode,
};

use crate::types::{
    AccountProviderConnectionResult, AccountProviderConnectionSettings,
    AccountProviderConnectionStatus, AccountProviderFamilyAvailabilityInput,
    AccountProviderOverlayError, CodingPlanAvailabilityProvider, CodingPlanAvailabilityResult,
    CodingPlanUnavailableReason, PlanKind, ProviderFamilyConnectionSelection, ProviderFamilyDomain,
};

/// 注入点的异步返回值。`Send` 是硬约束：这条链会被 Mode agent 交付路径
/// （`ProviderSource::read`）跨线程驱动，非 Send 的 Future 无法进入。
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// 与 TS `AccountProviderConnectionResolverOptions` 对应的注入点。
pub trait AccountProviderOverlayOptions {
    fn read_settings(
        &self,
    ) -> BoxFuture<'_, Result<AccountProviderConnectionSettings, AccountProviderOverlayError>>;

    fn load_account_identity(
        &self,
        family: ProviderFamilyDomain,
    ) -> BoxFuture<'_, Result<Option<String>, AccountProviderOverlayError>>;

    fn load_coding_plan_api_key<'a>(
        &'a self,
        provider_id: &'a str,
        family: ProviderFamilyDomain,
        account_identity: &'a str,
        force_refresh: bool,
    ) -> BoxFuture<'a, Result<Option<String>, AccountProviderOverlayError>>;

    fn resolve_family_availability<'a>(
        &'a self,
        input: AccountProviderFamilyAvailabilityInput,
    ) -> BoxFuture<
        'a,
        Result<IndexMap<String, CodingPlanAvailabilityResult>, AccountProviderOverlayError>,
    >;
}

const FAMILY_ORDER: [ProviderFamilyDomain; 2] =
    [ProviderFamilyDomain::Zai, ProviderFamilyDomain::Bigmodel];

const START_PLAN_SELECTION: ProviderFamilyConnectionSelection =
    ProviderFamilyConnectionSelection::StartPlan;

pub struct AccountProviderConnectionResolver<O> {
    options: O,
    previous_scopes: HashMap<String, String>,
}

impl<O: AccountProviderOverlayOptions + Send + Sync> AccountProviderConnectionResolver<O> {
    pub fn new(options: O) -> Self {
        Self {
            options,
            previous_scopes: HashMap::new(),
        }
    }

    pub fn options(&self) -> &O {
        &self.options
    }

    pub async fn resolve(
        &mut self,
        configured_providers: &ProviderConfigMap,
        reasons: &[String],
    ) -> Result<Vec<AccountProviderConnectionResult>, AccountProviderOverlayError> {
        let options = &self.options;
        let settings = options.read_settings().await?;
        let force_credential_refresh = reasons
            .iter()
            .any(|reason| is_credential_refresh_reason(reason));

        let mut identity_cache: IndexMap<
            ProviderFamilyDomain,
            Result<Option<String>, AccountProviderOverlayError>,
        > = IndexMap::new();
        let mut availability_by_provider_id: HashMap<String, CodingPlanAvailabilityResult> =
            HashMap::new();

        for family in FAMILY_ORDER {
            let mut configured: Vec<(&str, PlanKind)> = Vec::new();
            for (provider_id, rule) in configured_providers.entries() {
                let Some(access) = account_access(&rule.config) else {
                    continue;
                };
                let Some(account_type) = access.account_type.value().copied() else {
                    continue;
                };
                let Some(mode) = access.mode.value().copied() else {
                    continue;
                };
                if ProviderFamilyDomain::from_account_type(account_type) != family {
                    continue;
                }
                let Some(plan_kind) = PlanKind::from_mode(mode) else {
                    continue;
                };
                configured.push((provider_id.as_str(), plan_kind));
            }
            if configured.is_empty() {
                continue;
            }

            // 旧团队身份补全只限制付费访问，Start 只依赖当前登录账号。
            let unresolved = settings
                .unresolved_families
                .as_ref()
                .is_some_and(|families| families.contains(&family));
            let mut queryable: Vec<(&str, PlanKind)> = Vec::with_capacity(configured.len());
            for (provider_id, plan_kind) in configured {
                if unresolved && plan_kind != PlanKind::StartPlan {
                    availability_by_provider_id.insert(
                        provider_id.to_owned(),
                        CodingPlanAvailabilityResult::Unknown,
                    );
                    continue;
                }
                queryable.push((provider_id, plan_kind));
            }
            if queryable.is_empty() {
                continue;
            }

            let identity = load_identity_cached(options, &mut identity_cache, family).await?;
            let Some(identity) = identity else {
                for (provider_id, _) in queryable {
                    availability_by_provider_id.insert(
                        provider_id.to_owned(),
                        CodingPlanAvailabilityResult::Unavailable {
                            reason: CodingPlanUnavailableReason::CodingPlanNotConnected,
                        },
                    );
                }
                continue;
            };

            let mut availability_providers = Vec::with_capacity(queryable.len());
            for (provider_id, plan_kind) in &queryable {
                let api_key = if *plan_kind != PlanKind::TeamCodingPlan {
                    options
                        .load_coding_plan_api_key(
                            provider_id,
                            family,
                            &identity,
                            force_credential_refresh,
                        )
                        .await
                        .unwrap_or(None)
                } else {
                    None
                };
                availability_providers.push(CodingPlanAvailabilityProvider {
                    provider_id: (*provider_id).to_owned(),
                    family,
                    plan_kind: *plan_kind,
                    api_key,
                });
            }

            let resolved = options
                .resolve_family_availability(AccountProviderFamilyAvailabilityInput {
                    family,
                    providers: availability_providers,
                    selections: settings.selections.clone(),
                })
                .await?;
            for (provider_id, _) in queryable {
                availability_by_provider_id.insert(
                    provider_id.to_owned(),
                    resolved
                        .get(provider_id)
                        .cloned()
                        .unwrap_or(CodingPlanAvailabilityResult::Unknown),
                );
            }
        }

        let mut connections: Vec<AccountProviderConnectionResult> = Vec::new();
        let mut scopes: HashMap<String, String> = HashMap::new();
        for (provider_id, rule) in configured_providers.entries() {
            let Some(access) = account_access(&rule.config) else {
                continue;
            };
            let Some(account_type) = access.account_type.value().copied() else {
                connections.push(AccountProviderConnectionResult::simple(
                    provider_id.clone(),
                    AccountProviderConnectionStatus::Unavailable,
                ));
                continue;
            };
            let Some(mode) = access.mode.value().copied() else {
                connections.push(AccountProviderConnectionResult::simple(
                    provider_id.clone(),
                    AccountProviderConnectionStatus::Unavailable,
                ));
                continue;
            };
            let family = ProviderFamilyDomain::from_account_type(account_type);
            let selection = settings.selections.get(family);
            let identity = load_identity_cached(options, &mut identity_cache, family).await?;

            // last-known-good 只对同账号、同 Team 身份成立。
            let team_scope = if mode == ZhipuAccountMode::TeamCodingPlan {
                selection.and_then(|selection| selection.team_scope())
            } else {
                None
            };
            let scope = serde_json::to_string(&(identity.as_deref(), team_scope))
                .expect("scope 序列化不会失败");
            let reset_previous = self
                .previous_scopes
                .get(provider_id)
                .is_some_and(|previous| previous != &scope);
            scopes.insert(provider_id.clone(), scope);

            if mode == ZhipuAccountMode::OffPeak {
                let selected_plan_kind =
                    selection.map(ProviderFamilyConnectionSelection::plan_mode);
                let matching_plan_available = settings.provider_family_domain == Some(family)
                    && matches!(
                        selected_plan_kind,
                        Some(
                            ZhipuAccountMode::IndividualCodingPlan
                                | ZhipuAccountMode::TeamCodingPlan
                        )
                    )
                    && configured_providers
                        .entries()
                        .any(|(candidate_id, candidate)| {
                            let Some(candidate_access) = account_access(&candidate.config) else {
                                return false;
                            };
                            candidate_access.account_type.value() == Some(&account_type)
                                && candidate_access.mode.value().copied() == selected_plan_kind
                                && availability_by_provider_id.get(candidate_id).is_some_and(
                                    |availability| {
                                        availability.kind()
                                            == AccountProviderConnectionStatus::Available
                                    },
                                )
                        });
                connections.push(AccountProviderConnectionResult::simple(
                    provider_id.clone(),
                    if matching_plan_available {
                        AccountProviderConnectionStatus::Available
                    } else {
                        AccountProviderConnectionStatus::Unavailable
                    },
                ));
                continue;
            }

            let availability = availability_by_provider_id
                .get(provider_id)
                .cloned()
                .unwrap_or(CodingPlanAvailabilityResult::Unknown);
            let status = availability.kind();
            let current = settings.provider_family_domain == Some(family)
                && match mode {
                    ZhipuAccountMode::StartPlan => identity.is_some(),
                    other => {
                        selection.map(ProviderFamilyConnectionSelection::plan_mode) == Some(other)
                    }
                };
            let key_selection = if mode == ZhipuAccountMode::StartPlan {
                Some(&START_PLAN_SELECTION)
            } else {
                selection
            };
            let key_source =
                serde_json::to_string(&(identity.as_deref(), account_type.as_str(), key_selection))
                    .expect("connectionKey 序列化不会失败");

            let mut connection =
                AccountProviderConnectionResult::simple(provider_id.clone(), status);
            if status == AccountProviderConnectionStatus::Unavailable {
                connection.unavailable_reason = availability.reason().map(map_unavailable_reason);
            }
            connection.current = Some(current);
            connection.connection_key = Some(sha256_hex(key_source.as_bytes()));
            connection.models = availability.models().map(|models| models.to_vec());
            connection.effective_at = availability.effective_at();
            connection.reset_previous = reset_previous;
            connections.push(connection);
        }

        // 权益查询可能跨越切账号/套餐；发布前核对本轮作用域，失败时也不推进
        // previousScopes，否则下一轮会把未发布的账号误认作 last-known-good。
        let mut pending_identities = Vec::with_capacity(identity_cache.len());
        for family in identity_cache.keys() {
            pending_identities.push((*family, options.load_account_identity(*family)));
        }
        let mut identities_unchanged = true;
        for (family, pending) in pending_identities {
            let fresh = normalize_identity(pending.await?);
            let captured = identity_cache
                .get(&family)
                .expect("identity 缓存项存在")
                .clone()?;
            if captured != fresh {
                identities_unchanged = false;
            }
        }
        if !identities_unchanged {
            return Err(stale_resolution_error());
        }
        let settings_after = options.read_settings().await?;
        if settings_after != settings {
            return Err(stale_resolution_error());
        }

        self.previous_scopes = scopes;
        Ok(connections)
    }
}

fn account_access(config: &ProviderConfig) -> Option<&ZhipuAccountAccess> {
    match &config.access {
        Field::Value(ProviderAccess::ZhipuAccount(access)) => Some(access),
        _ => None,
    }
}

async fn load_identity_cached<O: AccountProviderOverlayOptions + Sync>(
    options: &O,
    cache: &mut IndexMap<ProviderFamilyDomain, Result<Option<String>, AccountProviderOverlayError>>,
    family: ProviderFamilyDomain,
) -> Result<Option<String>, AccountProviderOverlayError> {
    if let Some(cached) = cache.get(&family) {
        return cached.clone();
    }
    let value = options
        .load_account_identity(family)
        .await
        .map(normalize_identity);
    cache.insert(family, value.clone());
    value
}

fn normalize_identity(identity: Option<String>) -> Option<String> {
    identity
        .map(|identity| identity.trim().to_owned())
        .filter(|identity| !identity.is_empty())
}

fn is_credential_refresh_reason(reason: &str) -> bool {
    reason.contains("oauth-callback")
        || reason.split(':').next_back() == Some("oauth-login-entitlement")
}

fn stale_resolution_error() -> AccountProviderOverlayError {
    AccountProviderOverlayError::new("账号查询期间连接或身份发生变化，丢弃过期结果")
}

fn map_unavailable_reason(reason: CodingPlanUnavailableReason) -> AccountProviderUnavailableReason {
    match reason {
        CodingPlanUnavailableReason::CodingPlanNotAuthenticated => {
            AccountProviderUnavailableReason::NotAuthenticated
        }
        CodingPlanUnavailableReason::CodingPlanNotConnected => {
            AccountProviderUnavailableReason::NotConnected
        }
        CodingPlanUnavailableReason::CodingPlanAuthFailed => {
            AccountProviderUnavailableReason::CredentialFailed
        }
        CodingPlanUnavailableReason::CodingPlanNotEntitled => {
            AccountProviderUnavailableReason::NotEntitled
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
