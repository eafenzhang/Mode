//! Provider resolver 契约测试：覆盖 TS `resolver.ts` 的关键判定。

use indexmap::IndexMap;
use provider_config::{
    Field, ModelConfigRules, ProviderAccess, ProviderConfig, ProviderConfigMap, ProviderConfigRule,
    ProviderGroup, ProviderTemplate, ProviderTemplateMap, ProviderTemplateNameMap,
    ProviderVisibility, ZhipuAccountAccess,
};
use provider_resolver::{ProviderConfigResolver, ProviderConfigResolverInput, ProviderModelSource};

fn empty_config() -> ProviderConfig {
    ProviderConfig::default()
}

fn config_with_members(
    group: Field<ProviderGroup>,
    visibility: Field<ProviderVisibility>,
    builtin_ids: &[&str],
    personal_ids: &[&str],
    order: &[&str],
) -> ProviderConfig {
    ProviderConfig {
        group,
        visibility,
        builtin_model_ids: if builtin_ids.is_empty() {
            Field::Absent
        } else {
            Field::Value(builtin_ids.iter().map(|id| (*id).to_string()).collect())
        },
        personal_model_ids: if personal_ids.is_empty() {
            Field::Absent
        } else {
            Field::Value(personal_ids.iter().map(|id| (*id).to_string()).collect())
        },
        model_order: if order.is_empty() {
            Field::Absent
        } else {
            Field::Value(order.iter().map(|id| (*id).to_string()).collect())
        },
        ..ProviderConfig::default()
    }
}

fn rule(
    provider_id: &str,
    enabled: Field<bool>,
    config: ProviderConfig,
) -> (String, ProviderConfigRule) {
    (
        provider_id.to_string(),
        ProviderConfigRule {
            provider_id: provider_id.to_string(),
            template_id: Field::Absent,
            provider_name: Field::Value(format!("name-{provider_id}")),
            enabled,
            config,
        },
    )
}

fn map(entries: Vec<(String, ProviderConfigRule)>) -> ProviderConfigMap {
    ProviderConfigMap {
        values: entries.into_iter().collect::<IndexMap<_, _>>(),
    }
}

fn empty_rules() -> ModelConfigRules {
    ModelConfigRules::empty()
}

fn zhipu_access() -> Field<ProviderAccess> {
    Field::Value(ProviderAccess::ZhipuAccount(ZhipuAccountAccess {
        entitled: Field::Value(true),
        ..ZhipuAccountAccess::default()
    }))
}

fn resolve<'a>(
    builtin: &'a ProviderConfigMap,
    personal: &'a ProviderConfigMap,
    account: &'a ProviderConfigMap,
    rules: &'a ModelConfigRules,
    templates: Option<&'a ProviderTemplateMap>,
    states: Option<&'a provider_config::AccountProviderStates>,
    order: Option<&'a [String]>,
) -> provider_resolver::ProviderConfigResolution {
    ProviderConfigResolver.resolve(&ProviderConfigResolverInput {
        mode_builtin_providers: builtin,
        mode_builtin_provider_templates: templates,
        personal_providers: personal,
        mode_builtin_model_rules: rules,
        personal_models: rules,
        account_providers: account,
        account_states: states,
        personal_provider_order: order,
    })
}

#[test]
fn account_overlay_applies_only_to_builtin_ids_and_strips_group() {
    let builtin = map(vec![rule(
        "p1",
        Field::Absent,
        config_with_members(
            Field::Value(ProviderGroup::StandardPersonal),
            Field::Absent,
            &[],
            &[],
            &[],
        ),
    )]);
    let account = map(vec![
        rule(
            "p1",
            Field::Absent,
            config_with_members(
                Field::Value(ProviderGroup::ZaiFamily),
                Field::Absent,
                &[],
                &[],
                &[],
            ),
        ),
        rule("unknown", Field::Absent, empty_config()),
    ]);
    let empty_personal = map(vec![]);
    let empty_account_rules = empty_rules();
    let resolution = resolve(
        &builtin,
        &empty_personal,
        &account,
        &empty_account_rules,
        None,
        None,
        None,
    );
    // 账号覆盖去分组：effective builtin 的 group 仍来自 base（overlay 后 ZaiFamily? 不：
    // withoutGroup 先清空，overlay 到 builtin 的 group=btn base… base 有 StandardPersonal，
    // 覆盖层的 group 为空 → 保留 base 值）。
    let effective = resolution
        .effective_builtin_providers
        .get("p1")
        .expect("p1");
    assert_eq!(
        effective.config.group,
        Field::Value(ProviderGroup::StandardPersonal)
    );
    // 未知 id 的账号覆盖不进入 effective。
    assert!(resolution
        .effective_builtin_providers
        .get("unknown")
        .is_none());
}

#[test]
fn template_config_overlays_concrete_before_use() {
    let mut builtin_rule = rule(
        "p1",
        Field::Absent,
        config_with_members(Field::Absent, Field::Absent, &[], &[], &[]),
    )
    .1;
    builtin_rule.template_id = Field::Value("tpl".to_string());
    let builtin = map(vec![("p1".to_string(), builtin_rule)]);
    let templates = ProviderTemplateMap {
        values: IndexMap::from([(
            "tpl".to_string(),
            ProviderTemplate {
                template_id: "tpl".to_string(),
                template_name_map: ProviderTemplateNameMap::default(),
                config: ProviderConfig {
                    visibility: Field::Value(ProviderVisibility::Hidden),
                    ..ProviderConfig::default()
                },
            },
        )]),
    };
    let empty = map(vec![]);
    let rules = empty_rules();
    let resolution = resolve(
        &builtin,
        &empty,
        &empty,
        &rules,
        Some(&templates),
        None,
        None,
    );
    let resolved = resolution
        .resolved_providers
        .iter()
        .find(|provider| provider.provider_id == "p1")
        .expect("p1");
    // 模板 visibility=hidden 叠加到 concrete（concrete 缺席），模板配置也会带回 resolved。
    assert_eq!(
        resolved.config.visibility,
        Field::Value(ProviderVisibility::Hidden)
    );
    assert!(resolved.template_config.is_some());
}

#[test]
fn missing_template_adds_issue_and_blocks_registry() {
    let mut builtin_rule = rule(
        "p1",
        Field::Absent,
        config_with_members(Field::Absent, Field::Absent, &[], &[], &[]),
    )
    .1;
    builtin_rule.template_id = Field::Value("ghost".to_string());
    let builtin = map(vec![("p1".to_string(), builtin_rule)]);
    let empty = map(vec![]);
    let rules = empty_rules();
    let resolution = resolve(&builtin, &empty, &empty, &rules, None, None, None);
    assert!(resolution
        .issues
        .iter()
        .any(|issue| issue.code == "missing-template"));
    assert!(resolution.registry_providers.is_empty());
}

#[test]
fn provider_order_puts_family_first_then_builtin_then_personal() {
    let builtin = map(vec![
        rule(
            "btn-a",
            Field::Absent,
            config_with_members(Field::Absent, Field::Absent, &[], &[], &[]),
        ),
        rule(
            "fam",
            Field::Absent,
            config_with_members(
                Field::Value(ProviderGroup::ZaiFamily),
                Field::Absent,
                &[],
                &[],
                &[],
            ),
        ),
    ]);
    let personal = map(vec![rule(
        "own",
        Field::Absent,
        config_with_members(Field::Absent, Field::Absent, &[], &[], &[]),
    )]);
    let empty = map(vec![]);
    let rules = empty_rules();
    let order = vec!["own".to_string()];
    let resolution = resolve(
        &builtin,
        &personal,
        &empty,
        &rules,
        None,
        None,
        Some(&order),
    );
    let ids: Vec<&str> = resolution
        .resolved_providers
        .iter()
        .map(|provider| provider.provider_id.as_str())
        .collect();
    // family 永远最前；requestedOrder 把 personal 提到 builtin 之后、未排序 personal 之前。
    assert_eq!(ids, vec!["fam", "btn-a", "own"]);
}

#[test]
fn zhipu_account_is_always_enabled_and_hidden_blocks_selectable() {
    let mut zhipu_config = config_with_members(
        Field::Absent,
        Field::Value(ProviderVisibility::Hidden),
        &["m1"],
        &[],
        &[],
    );
    zhipu_config.access = zhipu_access();
    let builtin = map(vec![rule("acc", Field::Value(false), zhipu_config)]);
    let empty = map(vec![]);
    let rules = empty_rules();
    let resolution = resolve(&builtin, &empty, &empty, &rules, None, None, None);
    let resolved = &resolution.resolved_providers[0];
    assert!(resolved.enabled, "账号 Provider 不受 enabled=false 影响");
    assert_eq!(resolved.models.len(), 1);
    assert_eq!(resolved.models[0].source, ProviderModelSource::Builtin);
    assert!(!resolved.models[0].selectable, "hidden 不可选");
}

#[test]
fn account_state_current_false_blocks_executable() {
    let builtin = map(vec![rule(
        "p1",
        Field::Absent,
        config_with_members(Field::Absent, Field::Absent, &["m1"], &[], &[]),
    )]);
    let empty = map(vec![]);
    let rules = empty_rules();
    let states = provider_config::AccountProviderStates::from([(
        "p1".to_string(),
        provider_config::AccountProviderState {
            availability: provider_config::AccountProviderAvailability::Available,
            entitled: true,
            current: Some(false),
            unavailable_reason: None,
            connection_key: None,
            effective_at: None,
        },
    )]);
    let resolution = resolve(&builtin, &empty, &empty, &rules, None, Some(&states), None);
    let resolved = &resolution.resolved_providers[0];
    // 非当前账号：保留展示，但不向 Registry 发布模型。
    assert!(resolved.models.iter().all(|model| !model.executable));
    assert!(resolution.registry_providers.is_empty());
}

#[test]
fn personal_same_id_as_builtin_loses_group_and_appears_once() {
    let builtin = map(vec![rule(
        "p1",
        Field::Absent,
        config_with_members(Field::Absent, Field::Absent, &[], &[], &[]),
    )]);
    let personal = map(vec![rule(
        "p1",
        Field::Absent,
        config_with_members(
            Field::Value(ProviderGroup::ZaiFamily),
            Field::Absent,
            &[],
            &[],
            &[],
        ),
    )]);
    let empty = map(vec![]);
    let rules = empty_rules();
    let resolution = resolve(&builtin, &personal, &empty, &rules, None, None, None);
    assert_eq!(resolution.resolved_providers.len(), 1);
    // personal 去分组后叠加，分组仍来自 builtin（absent）。
    assert_eq!(resolution.resolved_providers[0].config.group, Field::Absent);
}
