//! 链路测试支撑：临时配置根与可注入的账号事实。

#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use account_provider_overlay::{
    AccountProviderConnectionSettings, AccountProviderOverlayError,
    AccountProviderOverlayOptions, AccountProviderFamilyAvailabilityInput, BoxFuture,
    CodingPlanAvailabilityResult, ProviderFamilyConnectionSelection,
    ProviderFamilyConnectionSelectionSettings, ProviderFamilyDomain,
};
use indexmap::IndexMap;

pub const REVISION: i64 = 7;

pub fn release() -> String {
    format!(
        r#"{{"schemaVersion":1,"revision":{REVISION},"config":{{"providerConfigRules":{{"templateRules":[],"providerRules":[
            {{"providerId":"zai-start","config":{{"group":"zai-family","access":{{"type":"zhipu-account","accountType":"zai","mode":"start-plan"}}}}}},
            {{"providerId":"zai-individual","config":{{"group":"zai-family","access":{{"type":"zhipu-account","accountType":"zai","mode":"individual-coding-plan"}}}}}}
        ]}},"modelConfigRules":{{"modelRules":[],"modelApiRules":[],"providerSiteRules":[],"templateModelRules":[],"builtinProviderModelRules":[]}}}}}}"#
    )
}

pub struct Fixture {
    pub dir: std::path::PathBuf,
    pub bundled: String,
    pub personal: std::path::PathBuf,
}

impl Fixture {
    pub fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "provider-chain-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let bundled = dir.join("mode-builtin.json");
        std::fs::write(&bundled, release()).unwrap();
        let personal = dir.join("provider_config.json");
        Self {
            bundled: bundled.to_string_lossy().into_owned(),
            personal,
            dir,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

pub struct FakeOptions {
    pub settings: AccountProviderConnectionSettings,
    pub identity: Option<String>,
    pub api_key: Option<String>,
    pub fail: Arc<AtomicBool>,
    pub availability: IndexMap<String, CodingPlanAvailabilityResult>,
}

impl FakeOptions {
    pub fn ready() -> Self {
        let mut availability = IndexMap::new();
        availability.insert(
            "zai-start".to_owned(),
            CodingPlanAvailabilityResult::Available { models: None },
        );
        availability.insert(
            "zai-individual".to_owned(),
            CodingPlanAvailabilityResult::Available {
                models: Some(vec!["glm-4.6".to_owned()]),
            },
        );
        Self {
            settings: AccountProviderConnectionSettings {
                provider_family_domain: Some(ProviderFamilyDomain::Zai),
                selections: ProviderFamilyConnectionSelectionSettings {
                    zai: Some(ProviderFamilyConnectionSelection::IndividualCodingPlan),
                    bigmodel: None,
                },
                unresolved_families: None,
            },
            identity: Some("account-1".to_owned()),
            api_key: Some("key-1".to_owned()),
            fail: Arc::new(AtomicBool::new(false)),
            availability,
        }
    }

    pub fn failing() -> Self {
        let options = Self::ready();
        options.fail.store(true, Ordering::SeqCst);
        options
    }
}

impl AccountProviderOverlayOptions for FakeOptions {
    fn read_settings(
        &self,
    ) -> BoxFuture<'_, Result<AccountProviderConnectionSettings, AccountProviderOverlayError>> {
        let settings = self.settings.clone();
        Box::pin(async move { Ok(settings) })
    }

    fn load_account_identity(
        &self,
        _family: ProviderFamilyDomain,
    ) -> BoxFuture<'_, Result<Option<String>, AccountProviderOverlayError>> {
        let identity = self.identity.clone();
        Box::pin(async move { Ok(identity) })
    }

    fn load_coding_plan_api_key<'a>(
        &'a self,
        _provider_id: &'a str,
        _family: ProviderFamilyDomain,
        _account_identity: &'a str,
        _force_refresh: bool,
    ) -> BoxFuture<'a, Result<Option<String>, AccountProviderOverlayError>> {
        let api_key = self.api_key.clone();
        Box::pin(async move { Ok(api_key) })
    }

    fn resolve_family_availability<'a>(
        &'a self,
        _input: AccountProviderFamilyAvailabilityInput,
    ) -> BoxFuture<
        'a,
        Result<IndexMap<String, CodingPlanAvailabilityResult>, AccountProviderOverlayError>,
    > {
        let fail = self.fail.load(Ordering::SeqCst);
        let availability = self.availability.clone();
        Box::pin(async move {
            if fail {
                return Err(AccountProviderOverlayError::new("availability boom"));
            }
            Ok(availability)
        })
    }
}
