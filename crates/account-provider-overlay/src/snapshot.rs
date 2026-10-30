//! Account Provider Config Snapshot 与 revision。
//!
//! 真源：`packages/provider/src/sources.ts` 的
//! `createAccountProviderConfigSnapshot` / `createFailClosedAccountProviderConfigSnapshot`。

use provider_config::{AccountProviderStates, Field, JsonValue, ProviderConfigMap};

use crate::config::AccountProviderConfigResolver;
use crate::connection::AccountProviderOverlayOptions;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountProviderConfigSnapshot {
    pub revision: String,
    pub based_on_mode_builtin_revision: String,
    pub providers: ProviderConfigMap,
    pub states: Option<AccountProviderStates>,
}

impl AccountProviderConfigSnapshot {
    pub fn to_json(&self) -> JsonValue {
        let mut entries = vec![
            (
                "revision".to_owned(),
                JsonValue::String(self.revision.clone()),
            ),
            (
                "basedOnModeBuiltinRevision".to_owned(),
                JsonValue::String(self.based_on_mode_builtin_revision.clone()),
            ),
            ("providers".to_owned(), self.providers.to_json()),
        ];
        if let Some(states) = &self.states {
            // 经 JsonValue 反序列化一次以保留 IndexMap 插入顺序（serde_json::Value
            // 的默认 Map 会按 BTreeMap 排序）。
            let states_json = serde_json::to_string(states).expect("states 序列化不会失败");
            entries.push((
                "states".to_owned(),
                JsonValue::from_json_str(&states_json).expect("states 反序列化不会失败"),
            ));
        }
        JsonValue::object_from(entries)
    }
}

/// revision 公式与 TS 完全一致：
///
/// `account:${JSON.stringify([basedOn, providers.toJSON(), states])}`
///
/// `states` 缺省时数组第三项仍是 `null`，但快照对象不携带 `states` 字段。
pub fn create_account_provider_config_snapshot(
    based_on_mode_builtin_revision: &str,
    providers: ProviderConfigMap,
    states: Option<AccountProviderStates>,
) -> AccountProviderConfigSnapshot {
    let payload = serde_json::to_string(&(
        based_on_mode_builtin_revision,
        providers.to_json(),
        states.as_ref(),
    ))
    .expect("revision 序列化不会失败");
    AccountProviderConfigSnapshot {
        revision: format!("account:{payload}"),
        based_on_mode_builtin_revision: based_on_mode_builtin_revision.to_owned(),
        providers,
        states,
    }
}

/// 首次账号事实尚未到达时，基于当前 Built-in 生成可发布的 fail-closed Overlay。
pub fn create_fail_closed_account_provider_config_snapshot(
    based_on_mode_builtin_revision: &str,
    builtin_providers: &ProviderConfigMap,
) -> AccountProviderConfigSnapshot {
    let mut unentitled = Vec::new();
    for (provider_id, rule) in builtin_providers.entries() {
        let is_account = matches!(
            &rule.config.access,
            Field::Value(provider_config::ProviderAccess::ZhipuAccount(_))
        );
        if !is_account {
            continue;
        }
        let config = provider_config::ProviderConfig {
            access: Field::Value(provider_config::ProviderAccess::ZhipuAccount(
                provider_config::ZhipuAccountAccess {
                    account_type: Field::Absent,
                    mode: Field::Absent,
                    entitled: Field::Value(false),
                },
            )),
            ..provider_config::ProviderConfig::default()
        };
        unentitled.push((
            provider_id.clone(),
            provider_config::ProviderConfigRule {
                provider_id: provider_id.clone(),
                template_id: Field::Absent,
                provider_name: Field::Absent,
                enabled: Field::Absent,
                config,
            },
        ));
    }
    create_account_provider_config_snapshot(
        based_on_mode_builtin_revision,
        ProviderConfigMap {
            values: unentitled.into_iter().collect(),
        },
        None,
    )
}

/// 便捷组合：连接解析 + config/states 投影 + snapshot。
pub struct AccountProviderConfigSource<O> {
    resolver: AccountProviderConfigResolver<O>,
}

impl<O: AccountProviderOverlayOptions> AccountProviderConfigSource<O> {
    pub fn new(options: O) -> Self {
        Self {
            resolver: AccountProviderConfigResolver::new(options),
        }
    }

    pub async fn resolve_snapshot(
        &mut self,
        input: crate::types::AccountProviderResolveInput<'_>,
    ) -> Result<AccountProviderConfigSnapshot, crate::types::AccountProviderOverlayError> {
        let output = self.resolver.resolve(input).await?;
        Ok(create_account_provider_config_snapshot(
            input.config_revision,
            output.providers,
            Some(output.states),
        ))
    }
}
