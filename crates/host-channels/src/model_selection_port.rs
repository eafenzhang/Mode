//! `ModelSelectionPort` 生产实现（automation 用）：Provider 配置 → 选择解析。
//!
//! 真源：`packages/provider/src/registry.ts`（validateModelSelectionOptions）、
//! `model-selection-config.ts`（resolveInitialModelSelection /
//! completeNewModelSelection）与 `effective-model-selection.ts`
//! （resolveEffectiveModelSelection）的 Host 消费子集。
//!
//! 范围：只输出 automation 需要的 `{selectionIssue, effectiveSelection,
//! preferredSelection}`。账号 provider 分类（account-plan/account-offpeak）与
//! legacy reasoning 归一化尚未迁——当前按 ordinary 处理，差异登记 spec。

use std::sync::Arc;

use automation_repo::ModelSelection;
use automation_runtime::{ModelSelectionPort, ModelSelectionView, RuntimeFuture};
use provider_config::{Field, ProviderConfigMap, ProviderVisibility};
use provider_config_source::ProviderConfigSnapshot;
use provider_resolver::{ProviderConfigResolver, ProviderConfigResolverInput};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionModel {
    pub model_id: String,
    pub reasoning_levels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionProvider {
    pub provider_id: String,
    pub hidden: bool,
    pub models: Vec<SelectionModel>,
}

/// Provider Registry 的选择子集（只含可执行 provider，按 resolver 顺序）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectionRegistry {
    pub providers: Vec<SelectionProvider>,
}

fn reasoning_levels(config: &provider_config::ModelConfig) -> Vec<String> {
    let Field::Value(specs) = &config.option_specs else {
        return Vec::new();
    };
    let Field::Value(reasoning) = &specs.reasoning_level else {
        return Vec::new();
    };
    match &reasoning.values {
        Field::Value(values) => values.clone(),
        _ => Vec::new(),
    }
}

fn provider_hidden(config: &provider_config::ProviderConfig) -> bool {
    matches!(&config.visibility, Field::Value(ProviderVisibility::Hidden))
}

/// resolver 输出 → 选择子集（`registry_providers` 即 TS Registry 的可执行子集）。
pub fn registry_from_snapshot(snapshot: &ProviderConfigSnapshot) -> SelectionRegistry {
    let account_providers = ProviderConfigMap::empty();
    let resolution = ProviderConfigResolver.resolve(&ProviderConfigResolverInput {
        mode_builtin_providers: &snapshot.mode_builtin_providers,
        mode_builtin_provider_templates: Some(&snapshot.mode_builtin_provider_templates),
        personal_providers: &snapshot.personal_providers,
        mode_builtin_model_rules: &snapshot.mode_builtin_model_rules,
        personal_models: &snapshot.personal_models,
        account_providers: &account_providers,
        account_states: None,
        personal_provider_order: Some(&snapshot.personal_provider_order),
    });
    SelectionRegistry {
        providers: resolution
            .registry_providers
            .iter()
            .map(|provider| SelectionProvider {
                provider_id: provider.provider_id.clone(),
                hidden: provider_hidden(&provider.config),
                models: provider
                    .models
                    .iter()
                    .map(|model| SelectionModel {
                        model_id: model.model_id.clone(),
                        reasoning_levels: reasoning_levels(&model.config),
                    })
                    .collect(),
            })
            .collect(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectionIssue {
    Missing,
    ProviderNotFound,
    ModelNotFound,
    ReasoningMissing,
    ReasoningNotSupported,
}

impl SelectionIssue {
    fn is_issue(self) -> bool {
        !matches!(self, Self::Missing)
    }
}

/// TS `resolveInitialModelSelection`：首个可见 provider/model 的最高档位推荐。
fn preferred_selection(registry: &SelectionRegistry) -> Option<ModelSelection> {
    for provider in &registry.providers {
        if provider.hidden {
            continue;
        }
        for model in &provider.models {
            let Some(level) = model.reasoning_levels.last() else {
                continue;
            };
            return Some(ModelSelection {
                provider_id: provider.provider_id.clone(),
                model_id: model.model_id.clone(),
                options: Some(automation_repo::SelectionOptions {
                    reasoning_level: Some(level.clone()),
                }),
            });
        }
    }
    None
}

/// TS `resolveEffectiveModelSelection`（ordinary provider；无 account overlay）。
fn effective_selection(
    registry: &SelectionRegistry,
    selection: &ModelSelection,
) -> (Option<ModelSelection>, SelectionIssue) {
    let Some(provider) = registry
        .providers
        .iter()
        .find(|provider| provider.provider_id == selection.provider_id)
    else {
        return (None, SelectionIssue::ProviderNotFound);
    };
    // ordinary provider：hidden 一律视为不可用（account-offpeak 例外未迁）。
    if provider.hidden {
        return (None, SelectionIssue::ProviderNotFound);
    }
    let Some(model) = provider
        .models
        .iter()
        .find(|model| model.model_id == selection.model_id)
    else {
        return (None, SelectionIssue::ModelNotFound);
    };
    let reasoning_level = selection
        .options
        .as_ref()
        .and_then(|options| options.reasoning_level.as_deref());
    let issue = match reasoning_level {
        None => Some(SelectionIssue::ReasoningMissing),
        Some(level) if !model.reasoning_levels.iter().any(|value| value == level) => {
            Some(SelectionIssue::ReasoningNotSupported)
        }
        Some(_) => None,
    };
    let effective = ModelSelection {
        provider_id: selection.provider_id.clone(),
        model_id: selection.model_id.clone(),
        options: match issue {
            None => selection.options.clone(),
            // 档位缺失/不支持时只保留模型身份，等待用户选择档位。
            Some(_) => None,
        },
    };
    (
        Some(effective),
        issue.unwrap_or(SelectionIssue::Missing),
    )
}

/// 纯解析入口（测试/复用）。
pub fn resolve_selection_view(
    registry: &SelectionRegistry,
    selection: Option<&ModelSelection>,
) -> ModelSelectionView {
    let preferred = preferred_selection(registry);
    match selection {
        None => ModelSelectionView {
            selection_issue: false,
            effective_selection: None,
            preferred_selection: preferred,
        },
        Some(selection) => {
            let (effective, issue) = effective_selection(registry, selection);
            ModelSelectionView {
                selection_issue: issue.is_issue(),
                effective_selection: effective,
                preferred_selection: preferred,
            }
        }
    }
}

/// `ModelSelectionPort` 生产实现：每次 getView 读取最新 provider 配置快照。
pub struct ProviderConfigModelSelectionPort {
    snapshot: Arc<dyn Fn() -> Result<ProviderConfigSnapshot, String> + Send + Sync>,
}

impl ProviderConfigModelSelectionPort {
    pub fn new(
        snapshot: Arc<dyn Fn() -> Result<ProviderConfigSnapshot, String> + Send + Sync>,
    ) -> Self {
        Self { snapshot }
    }
}

impl ModelSelectionPort for ProviderConfigModelSelectionPort {
    fn get_view(
        &self,
        selection: Option<ModelSelection>,
    ) -> RuntimeFuture<'_, Result<ModelSelectionView, String>> {
        Box::pin(async move {
            let snapshot = (self.snapshot)()?;
            let registry = registry_from_snapshot(&snapshot);
            Ok(resolve_selection_view(&registry, selection.as_ref()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(id: &str, hidden: bool, models: &[(&str, &[&str])]) -> SelectionProvider {
        SelectionProvider {
            provider_id: id.to_string(),
            hidden,
            models: models
                .iter()
                .map(|(model_id, levels)| SelectionModel {
                    model_id: model_id.to_string(),
                    reasoning_levels: levels.iter().map(|level| level.to_string()).collect(),
                })
                .collect(),
        }
    }

    fn selection(provider: &str, model: &str, level: Option<&str>) -> ModelSelection {
        ModelSelection {
            provider_id: provider.to_string(),
            model_id: model.to_string(),
            options: level.map(|level| automation_repo::SelectionOptions {
                reasoning_level: Some(level.to_string()),
            }),
        }
    }

    #[test]
    fn empty_snapshot_yields_empty_registry() {
        let snapshot = ProviderConfigSnapshot {
            revision: "[]".to_string(),
            mode_builtin_revision: String::new(),
            personal_revision: String::new(),
            mode_builtin_providers: ProviderConfigMap::empty(),
            mode_builtin_provider_templates: provider_config::ProviderTemplateMap::empty(),
            personal_providers: ProviderConfigMap::empty(),
            mode_builtin_model_rules: provider_config::ModelConfigRules::empty(),
            personal_models: provider_config::ModelConfigRules::empty(),
            personal_provider_order: Vec::new(),
        };
        let registry = registry_from_snapshot(&snapshot);
        assert!(registry.providers.is_empty());
        let view = resolve_selection_view(&registry, None);
        assert!(view.preferred_selection.is_none());
        assert!(!view.selection_issue);
    }

    #[test]
    fn preferred_skips_hidden_and_uses_highest_level() {
        let registry = SelectionRegistry {
            providers: vec![
                provider("hidden", true, &[("m", &["low"])]),
                provider("a", false, &[("no-levels", &[]), ("m1", &["low", "high"])]),
            ],
        };
        let view = resolve_selection_view(&registry, None);
        assert!(!view.selection_issue);
        assert!(view.effective_selection.is_none());
        let preferred = view.preferred_selection.expect("preferred");
        assert_eq!(preferred.provider_id, "a");
        assert_eq!(preferred.model_id, "m1");
        assert_eq!(
            preferred
                .options
                .as_ref()
                .and_then(|options| options.reasoning_level.as_deref()),
            Some("high")
        );
    }

    #[test]
    fn effective_selection_reports_issues() {
        let registry = SelectionRegistry {
            providers: vec![provider("a", false, &[("m1", &["low", "high"])])],
        };
        // 完整选择可用。
        let view = resolve_selection_view(&registry, Some(&selection("a", "m1", Some("low"))));
        assert!(!view.selection_issue);
        assert_eq!(
            view.effective_selection.expect("effective"),
            selection("a", "m1", Some("low"))
        );

        // 缺档位：issue，保留模型身份。
        let view = resolve_selection_view(&registry, Some(&selection("a", "m1", None)));
        assert!(view.selection_issue);
        let effective = view.effective_selection.expect("effective");
        assert_eq!(effective.provider_id, "a");
        assert!(effective.options.is_none());

        // 档位不支持。
        let view = resolve_selection_view(&registry, Some(&selection("a", "m1", Some("ultra"))));
        assert!(view.selection_issue);
        assert!(view
            .effective_selection
            .expect("effective")
            .options
            .is_none());

        // provider / model 不存在。
        let view = resolve_selection_view(&registry, Some(&selection("x", "m1", Some("low"))));
        assert!(view.selection_issue);
        assert!(view.effective_selection.is_none());
        let view = resolve_selection_view(&registry, Some(&selection("a", "x", Some("low"))));
        assert!(view.selection_issue);
        assert!(view.effective_selection.is_none());
    }

    #[test]
    fn hidden_provider_is_provider_not_found() {
        let registry = SelectionRegistry {
            providers: vec![provider("a", true, &[("m1", &["low"])])],
        };
        let view = resolve_selection_view(&registry, Some(&selection("a", "m1", Some("low"))));
        assert!(view.selection_issue);
        assert!(view.effective_selection.is_none());
        assert!(view.preferred_selection.is_none());
    }
}
