//! 与 TS 真源的对拍。
//!
//! fixture 由 `pnpm account-provider-overlay:fixtures` 生成：每条场景步骤保存
//! 输入、确定性桩序列与规范 JSON 原文（connections / providers / states /
//! revision / 调用日志）。本测试用相同输入与桩重放，逐字节比较。

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use account_provider_overlay::{
    create_account_provider_config_resolver, create_account_provider_config_snapshot,
    resolve_account_provider_configs, AccountProviderConnectionResult,
    AccountProviderConnectionSettings, AccountProviderFamilyAvailabilityInput,
    AccountProviderOverlayError, AccountProviderOverlayOptions, AccountProviderResolveInput,
    AccountProviderStates, BoxFuture, CodingPlanAvailabilityProvider, CodingPlanAvailabilityResult,
    ProviderConfigMap, ProviderFamilyDomain, ResolveAccountProviderConfigsInput,
};
use provider_config::{parse_provider_config_map, JsonValue};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// fixture 形状
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    version: u32,
    config_revision: String,
    scenarios: Vec<Scenario>,
    config_cases: Vec<ConfigCase>,
    snapshot_cases: Vec<SnapshotCase>,
}

#[derive(Debug, Deserialize)]
struct Scenario {
    name: String,
    steps: Vec<Step>,
    expect: Vec<StepExpect>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Step {
    settings: Vec<AccountProviderConnectionSettings>,
    #[serde(default)]
    identities: BTreeMap<String, Vec<Option<String>>>,
    #[serde(default)]
    api_keys: BTreeMap<String, Vec<JsonValue>>,
    #[serde(default)]
    availability: BTreeMap<String, Vec<BTreeMap<String, AvailabilityDirective>>>,
    #[serde(default)]
    reasons: Vec<String>,
    configured: Vec<JsonValue>,
    previous: Vec<JsonValue>,
    #[serde(default)]
    previous_states: Option<AccountProviderStates>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AvailabilityDirective {
    #[serde(default)]
    result: Option<CodingPlanAvailabilityResult>,
    #[serde(default)]
    api_keys_as_models: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StepExpect {
    connections: Option<String>,
    providers: Option<String>,
    states: Option<String>,
    revision: Option<String>,
    calls: String,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ConfigCase {
    name: String,
    configured: Vec<JsonValue>,
    previous: Vec<JsonValue>,
    connections: Vec<AccountProviderConnectionResult>,
    expect: ConfigExpect,
}

#[derive(Debug, Deserialize)]
struct ConfigExpect {
    providers: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotCase {
    name: String,
    based_on: String,
    providers: Vec<JsonValue>,
    #[serde(default)]
    states: Option<AccountProviderStates>,
    expect: SnapshotExpect,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotExpect {
    revision: String,
    has_states: bool,
    json: String,
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/account-provider-overlay.json")
}

fn load_fixture() -> Fixture {
    let text = std::fs::read_to_string(fixture_path()).expect("读取 fixture");
    let fixture: Fixture = serde_json::from_str(&text).expect("解析 fixture");
    assert_eq!(fixture.version, 1);
    fixture
}

fn parse_map(values: &[JsonValue]) -> ProviderConfigMap {
    parse_provider_config_map(&JsonValue::Array(values.to_vec()), &[])
        .expect("fixture 的 configured/previous 必须是合法 ProviderConfigMap")
}

// ---------------------------------------------------------------------------
// 确定性桩
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct CallLog {
    settings: usize,
    identities: Vec<(String, Option<String>)>,
    api_keys: Vec<(String, String, String, bool)>,
    availability: Vec<AvailabilityCall>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AvailabilityCall {
    family: String,
    providers: Vec<CodingPlanAvailabilityProvider>,
}

#[derive(Debug)]
enum KeyStep {
    Throw,
    Value(Option<String>),
}

struct ScriptedState {
    step: Step,
    settings_index: usize,
    identity_indices: BTreeMap<String, usize>,
    api_key_indices: BTreeMap<String, usize>,
    availability_indices: BTreeMap<String, usize>,
    calls: CallLog,
}

impl ScriptedState {
    fn new(step: Step) -> Self {
        Self {
            step,
            settings_index: 0,
            identity_indices: BTreeMap::new(),
            api_key_indices: BTreeMap::new(),
            availability_indices: BTreeMap::new(),
            calls: CallLog::default(),
        }
    }
}

#[derive(Clone)]
struct ScriptedOptions {
    state: Rc<RefCell<ScriptedState>>,
}

impl ScriptedOptions {
    fn new(step: Step) -> Self {
        Self {
            state: Rc::new(RefCell::new(ScriptedState::new(step))),
        }
    }

    fn reset(&self, step: Step) {
        *self.state.borrow_mut() = ScriptedState::new(step);
    }

    fn calls_json(&self) -> String {
        serde_json::to_string(&self.state.borrow().calls).expect("调用日志序列化不会失败")
    }
}

fn pick<T: Clone>(values: &[T], index: usize, label: &str) -> T {
    assert!(!values.is_empty(), "fixture 缺少 {label}");
    values[index.min(values.len() - 1)].clone()
}

fn key_step(value: &JsonValue) -> KeyStep {
    match value {
        JsonValue::Null => KeyStep::Value(None),
        JsonValue::String(text) if text == "<throw>" => KeyStep::Throw,
        JsonValue::String(text) => KeyStep::Value(Some(text.clone())),
        other => panic!("apiKeys 项必须是字符串或 null，实际为 {other:?}"),
    }
}

impl AccountProviderOverlayOptions for ScriptedOptions {
    fn read_settings(
        &self,
    ) -> BoxFuture<'_, Result<AccountProviderConnectionSettings, AccountProviderOverlayError>> {
        let value = {
            let mut state = self.state.borrow_mut();
            state.calls.settings += 1;
            let index = state.settings_index;
            let value = pick(&state.step.settings, index, "settings");
            state.settings_index += 1;
            value
        };
        Box::pin(async move { Ok(value) })
    }

    fn load_account_identity(
        &self,
        family: ProviderFamilyDomain,
    ) -> BoxFuture<'_, Result<Option<String>, AccountProviderOverlayError>> {
        let value = {
            let mut state = self.state.borrow_mut();
            let key = family.as_str().to_owned();
            let index = *state.identity_indices.get(&key).unwrap_or(&0);
            let values = state
                .step
                .identities
                .get(&key)
                .cloned()
                .unwrap_or_else(|| vec![None]);
            let value = pick(&values, index, &format!("identities.{key}"));
            *state.identity_indices.entry(key.clone()).or_insert(0) += 1;
            state.calls.identities.push((key, value.clone()));
            value
        };
        Box::pin(async move { Ok(value) })
    }

    fn load_coding_plan_api_key<'a>(
        &'a self,
        provider_id: &'a str,
        family: ProviderFamilyDomain,
        account_identity: &'a str,
        force_refresh: bool,
    ) -> BoxFuture<'a, Result<Option<String>, AccountProviderOverlayError>> {
        let outcome = {
            let mut state = self.state.borrow_mut();
            state.calls.api_keys.push((
                provider_id.to_owned(),
                family.as_str().to_owned(),
                account_identity.to_owned(),
                force_refresh,
            ));
            let index = *state.api_key_indices.get(provider_id).unwrap_or(&0);
            let values = state
                .step
                .api_keys
                .get(provider_id)
                .cloned()
                .unwrap_or_else(|| vec![JsonValue::Null]);
            let outcome = key_step(&pick(&values, index, &format!("apiKeys.{provider_id}")));
            *state
                .api_key_indices
                .entry(provider_id.to_owned())
                .or_insert(0) += 1;
            outcome
        };
        Box::pin(async move {
            match outcome {
                KeyStep::Throw => Err(AccountProviderOverlayError::new("stub api key failure")),
                KeyStep::Value(value) => Ok(value),
            }
        })
    }

    fn resolve_family_availability<'a>(
        &'a self,
        input: AccountProviderFamilyAvailabilityInput,
    ) -> BoxFuture<
        'a,
        Result<
            indexmap::IndexMap<String, CodingPlanAvailabilityResult>,
            AccountProviderOverlayError,
        >,
    > {
        let resolved = {
            let mut state = self.state.borrow_mut();
            let key = input.family.as_str().to_owned();
            state.calls.availability.push(AvailabilityCall {
                family: key.clone(),
                providers: input.providers.clone(),
            });
            let maps = state
                .step
                .availability
                .get(&key)
                .cloned()
                .unwrap_or_else(|| vec![BTreeMap::new()]);
            let index = *state.availability_indices.get(&key).unwrap_or(&0);
            let directives = pick(&maps, index, &format!("availability.{key}"));
            *state.availability_indices.entry(key).or_insert(0) += 1;

            let mut resolved = indexmap::IndexMap::new();
            for provider in &input.providers {
                let value = match directives.get(&provider.provider_id) {
                    None => CodingPlanAvailabilityResult::Unknown,
                    Some(directive) if directive.api_keys_as_models => {
                        CodingPlanAvailabilityResult::Available {
                            models: Some(vec![provider
                                .api_key
                                .clone()
                                .unwrap_or_else(|| "null".to_owned())]),
                        }
                    }
                    Some(directive) => directive
                        .result
                        .clone()
                        .unwrap_or(CodingPlanAvailabilityResult::Unknown),
                };
                resolved.insert(provider.provider_id.clone(), value);
            }
            resolved
        };
        Box::pin(async move { Ok(resolved) })
    }
}

// ---------------------------------------------------------------------------
// 最小 executor：桩 future 全部立即 ready。
// ---------------------------------------------------------------------------

fn block_on<F: Future>(future: F) -> F::Output {
    let mut context = Context::from_waker(Waker::noop());
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

// ---------------------------------------------------------------------------
// 对拍
// ---------------------------------------------------------------------------

#[test]
fn scenarios_match_ts_byte_for_byte() {
    let fixture = load_fixture();
    for scenario in &fixture.scenarios {
        assert_eq!(
            scenario.steps.len(),
            scenario.expect.len(),
            "{}: steps/expect 数量不一致",
            scenario.name
        );
        let options = ScriptedOptions::new(scenario.steps[0].clone());
        let handle = options.clone();
        let mut resolver = create_account_provider_config_resolver(options);

        for (step_index, (step, expect)) in scenario.steps.iter().zip(&scenario.expect).enumerate()
        {
            let label = format!("{}/step-{step_index}", scenario.name);
            handle.reset(step.clone());
            let configured = parse_map(&step.configured);
            let previous = parse_map(&step.previous);
            let input = AccountProviderResolveInput {
                config_revision: &fixture.config_revision,
                configured_providers: &configured,
                previous_providers: &previous,
                previous_states: step.previous_states.as_ref(),
                reasons: &step.reasons,
            };
            let outcome = block_on(resolver.resolve_with_connections(input));

            assert_eq!(
                handle.calls_json(),
                expect.calls,
                "{label}: 桩调用序列不一致"
            );

            match (&outcome, &expect.error) {
                (Ok(result), None) => {
                    assert_eq!(
                        serde_json::to_string(&result.connections).expect("connections"),
                        expect.connections.as_deref().expect("expect.connections"),
                        "{label}: connections 不一致"
                    );
                    assert_eq!(
                        result.providers.to_json().to_json_string(),
                        expect.providers.as_deref().expect("expect.providers"),
                        "{label}: providers 不一致"
                    );
                    assert_eq!(
                        serde_json::to_string(&result.states).expect("states"),
                        expect.states.as_deref().expect("expect.states"),
                        "{label}: states 不一致"
                    );
                    let snapshot = create_account_provider_config_snapshot(
                        &fixture.config_revision,
                        result.providers.clone(),
                        Some(result.states.clone()),
                    );
                    assert_eq!(
                        snapshot.revision,
                        expect.revision.as_deref().expect("expect.revision"),
                        "{label}: revision 不一致"
                    );
                }
                (Err(error), Some(expected)) => {
                    assert_eq!(error.message(), expected, "{label}: 错误原文不一致");
                }
                (Ok(_), Some(expected)) => {
                    panic!("{label}: 期望错误 {expected:?}，实际成功");
                }
                (Err(error), None) => {
                    panic!("{label}: 期望成功，实际错误 {error}");
                }
            }
        }
    }
}

#[test]
fn config_cases_match_ts_byte_for_byte() {
    let fixture = load_fixture();
    for case in &fixture.config_cases {
        let configured = parse_map(&case.configured);
        let previous = parse_map(&case.previous);
        let outcome = resolve_account_provider_configs(ResolveAccountProviderConfigsInput {
            configured_providers: &configured,
            previous_providers: &previous,
            connections: &case.connections,
        });
        match (&outcome, &case.expect.error) {
            (Ok(providers), None) => assert_eq!(
                providers.to_json().to_json_string(),
                case.expect.providers.as_deref().expect("expect.providers"),
                "{}: providers 不一致",
                case.name
            ),
            (Err(error), Some(expected)) => {
                assert_eq!(error.message(), expected, "{}: 错误原文不一致", case.name)
            }
            (Ok(_), Some(expected)) => {
                panic!("{}: 期望错误 {expected:?}，实际成功", case.name)
            }
            (Err(error), None) => {
                panic!("{}: 期望成功，实际错误 {error}", case.name)
            }
        }
    }
}

#[test]
fn snapshot_cases_match_ts_byte_for_byte() {
    let fixture = load_fixture();
    for case in &fixture.snapshot_cases {
        let providers = parse_map(&case.providers);
        let snapshot =
            create_account_provider_config_snapshot(&case.based_on, providers, case.states.clone());
        assert_eq!(
            snapshot.revision, case.expect.revision,
            "{}: revision",
            case.name
        );
        assert_eq!(
            snapshot.states.is_some(),
            case.expect.has_states,
            "{}: states 存在性",
            case.name
        );
        assert_eq!(
            snapshot.to_json().to_json_string(),
            case.expect.json,
            "{}: snapshot JSON",
            case.name
        );
    }
}
