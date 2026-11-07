//! Agent 运行时装配（S0 健康面 / S2 翻面的壳侧唯一所有者）。
//!
//! spec：`docs/specs/host-ownership-switch.md` §5。S0 只接健康只读面：
//! registry 的工厂在这里装配完整启动规格（env 覆盖 → dev dist/源码 → 打包
//! `mode.cjs` → 已部署 native binary 兜底，见 `agent-process::resolve_agent_command`
//! 与 `docs/specs/agent-process.md`），但只有 `agent-health` 频道消费它——
//! `list`/`describe` 都是只读，不会触发 spawn。S2 翻面时 `mode-agent` 频道
//! 复用同一个 registry，不另建第二所有者。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use agent_process::{
    resolve_agent_command, AgentControlClient, AgentProcessError, AgentResolveContext,
    AgentRuntimeRegistry, SpecFactory, OfficialMcpAuthHeadersFuture,
    OfficialMcpAuthPortFailureReason,
};
use account_api_key_resolver::{
    AccountApiHosts, AccountApiKeyResolver, ReqwestAccountApiHttp,
};
use account_provider_overlay::{
    AccountProviderConnectionSettings, ProviderFamilyConnectionSelectionSettings,
    ProviderFamilyDomain,
};
use coding_plan_http::ReqwestCodingPlanHttp;
use official_mcp_auth::IsOfficialMcpOriginTrustedInput;
use provider_account_facts::{
    AccountApiKeyExchange, AccountCredentialPort, AccountSettingsPort, EnvCodingPlanUrls,
    ProviderAccountFacts,
};
use provider_config_repository::{
    create_node_provider_runtime_path_env, resolve_node_provider_runtime_paths,
    ProviderRuntimePaths, PERSONAL_PROVIDER_CONFIG_FILE_NAME,
};
use provider_source_chain::AgentProviderSource;
use runtime_env::EnvMap;
use runtime_tools::{find_mode_agent_runtime_binary, AgentRuntimeContext, NativeToolFileSystem};
use subagents_service::{SubagentsService, SubagentsServiceOptions};

/// 壳侧 Agent 运行时配置（与 Node Host 同口径的资源定位输入）。
pub struct AgentRuntimeConfig {
    pub resources_dir: Option<PathBuf>,
    /// 数据根（workspace cwd 失效时的回落，与 Node Host 同树）。
    pub data_root: PathBuf,
    /// 应用版本（Start Plan balance URL 的 `app_version`）。
    pub app_version: String,
}

fn home_dir() -> PathBuf {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

fn node_candidates(resources_dir: &Option<PathBuf>) -> Vec<PathBuf> {
    let binary = if cfg!(windows) { "node.exe" } else { "node" };
    match resources_dir {
        Some(dir) => vec![dir.join("node").join(binary)],
        None => Vec::new(),
    }
}

/// 设置端口：读取 settings store 的账号连接字段。
struct ShellSettingsPort {
    settings: Arc<settings_store::SettingsService>,
}

impl AccountSettingsPort for ShellSettingsPort {
    fn read(&self) -> AccountProviderConnectionSettings {
        let document = self.settings.get();
        let provider_family_domain = document
            .get("providerFamilyDomain")
            .and_then(serde_json::Value::as_str)
            .and_then(|value| match value.trim() {
                "zai" => Some(ProviderFamilyDomain::Zai),
                "bigmodel" => Some(ProviderFamilyDomain::Bigmodel),
                _ => None,
            });
        let selections = document
            .get("providerFamilyConnectionSelections")
            .cloned()
            .and_then(|value| {
                serde_json::from_value::<ProviderFamilyConnectionSelectionSettings>(value).ok()
            })
            .unwrap_or_default();
        AccountProviderConnectionSettings {
            provider_family_domain,
            selections,
            unresolved_families: None,
        }
    }
}

/// 凭据端口：凭据文件读取失败按缺失处理（与 TS load 失败语义一致）。
struct ShellCredentialPort {
    store: credential_store::CredentialStore,
}

impl AccountCredentialPort for ShellCredentialPort {
    fn load(&self, key: &str) -> Option<String> {
        self.store.load(key).ok().flatten()
    }

    fn save(&self, key: &str, value: &str) -> Result<(), String> {
        self.store.save(key, value).map_err(|error| error.to_string())
    }
}

/// Provider 配置路径：Built-in 随包/开发树文件 + Personal `provider_config.json`。
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProviderPaths {
    bundled: String,
    personal: PathBuf,
}

fn find_bundled_provider_config(resources_dir: Option<&Path>, cwd: &Path) -> Option<PathBuf> {
    if let Some(resources) = resources_dir {
        let candidate = resources
            .join("config")
            .join("provider")
            .join("mode-builtin.json");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    cwd.ancestors()
        .map(|dir| dir.join("config").join("provider").join("mode-builtin.json"))
        .find(|candidate| candidate.is_file())
}

/// 与 TS `createNodeProviderRuntimePathEnv` 同口径：显式 env 优先，否则随包/开发树
/// Built-in + 数据根下的 Personal 文件。找不到 Built-in 时返回 `None`（不注入源，
/// 保持 agent 自行解析 env 的旧行为）。
fn resolve_provider_paths(
    resources_dir: Option<&Path>,
    data_root: &Path,
    cwd: &Path,
    env: &BTreeMap<String, String>,
) -> Option<ProviderPaths> {
    match resolve_node_provider_runtime_paths(env) {
        Ok(Some(paths)) => {
            return Some(ProviderPaths {
                bundled: paths.mode_builtin_file_path,
                personal: PathBuf::from(paths.personal_file_path),
            });
        }
        Ok(None) => {}
        Err(error) => crate::host::shell_log(&format!(
            "provider runtime path env ignored: {}",
            error.message()
        )),
    }
    let bundled = find_bundled_provider_config(resources_dir, cwd)?;
    Some(ProviderPaths {
        bundled: bundled.to_string_lossy().into_owned(),
        personal: data_root
            .join("v2")
            .join(PERSONAL_PROVIDER_CONFIG_FILE_NAME),
    })
}

/// 反向请求解析器：把 `requestProviderRuntimeHeaders` 交给账号事实层。
struct ShellRuntimeHeadersResolver {
    facts: Arc<ProviderAccountFacts<ReqwestCodingPlanHttp>>,
}

impl agent_process::ProviderRuntimeHeadersResolver for ShellRuntimeHeadersResolver {
    fn resolve<'a>(
        &'a self,
        params: &'a agent_protocol::provider_runtime_headers::ProviderRuntimeHeadersRequestParams,
    ) -> agent_process::ProviderRuntimeHeadersFuture<'a> {
        Box::pin(async move {
            let access = params.account_access.as_ref()?;
            self.facts
                .resolve_runtime_api_key(&params.provider_id, access)
                .await
        })
    }
}

struct ShellOfficialMcpAuthResolver {
    settings: Arc<settings_store::SettingsService>,
    credentials: credential_store::CredentialStore,
    mode_api_origin: String,
}

impl agent_process::OfficialMcpAuthHeadersResolver for ShellOfficialMcpAuthResolver {
    fn resolve<'a>(
        &'a self,
        params: &'a agent_protocol::official_mcp_auth::OfficialMcpAuthHeadersRequestParams,
    ) -> OfficialMcpAuthHeadersFuture<'a> {
        Box::pin(async move {
            let trust = official_mcp_auth::is_official_mcp_origin_trusted(
                &IsOfficialMcpOriginTrustedInput {
                    dev_trusted_origins_raw: None,
                    origin: params.target_origin.trim(),
                    plugin_id: params.plugin_id.trim(),
                    mode_api_origin: Some(self.mode_api_origin.as_str()),
                },
            );
            if !trust.trusted {
                return Err(OfficialMcpAuthPortFailureReason::OfficialMcpOriginUntrusted);
            }
            let active_provider = self
                .credentials
                .load("oauth:active_provider")
                .ok()
                .flatten()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .ok_or(OfficialMcpAuthPortFailureReason::OfficialAuthUnavailable)?;
            let jwt = self
                .credentials
                .load("zcodejwttoken")
                .ok()
                .flatten()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .ok_or(OfficialMcpAuthPortFailureReason::OfficialAuthUnavailable)?;
            let family = match active_provider.as_str() {
                "zai" => ProviderFamilyDomain::Zai,
                "bigmodel" => ProviderFamilyDomain::Bigmodel,
                _ => return Err(OfficialMcpAuthPortFailureReason::OfficialAuthUnavailable),
            };
            let settings = self.settings.get();
            let selections = settings
                .get("providerFamilyConnectionSelections")
                .cloned()
                .and_then(|value| {
                    serde_json::from_value::<
                        account_provider_overlay::ProviderFamilyConnectionSelectionSettings,
                    >(value)
                    .ok()
                })
                .unwrap_or_default();
            let team_scope = selections.get(family).and_then(|selection| {
                match selection {
                    account_provider_overlay::ProviderFamilyConnectionSelection::TeamCodingPlan {
                        organization_id,
                        project_id,
                        ..
                    } => Some((organization_id.clone(), project_id.clone())),
                    _ => None,
                }
            });
            let mut headers = std::collections::BTreeMap::new();
            headers.insert(
                "Authorization".to_string(),
                format!("Bearer {jwt}"),
            );
            // wireScope：BigModel 团队带 scope 头；ZAI 团队与 Off-Peak 一致为 null。
            if family == ProviderFamilyDomain::Bigmodel {
                if let Some((organization_id, project_id)) = team_scope {
                    headers.insert("Bigmodel-Target-Type".to_string(), "TEAM".to_string());
                    headers.insert("Bigmodel-Organization".to_string(), organization_id);
                    headers.insert("Bigmodel-Project".to_string(), project_id);
                }
            }
            Ok(headers)
        })
    }
}

fn attach_key_exchanges<P: coding_plan_http::CodingPlanHttpPort>(
    facts: ProviderAccountFacts<P>,
    api_http: ReqwestAccountApiHttp,
    hosts: AccountApiHosts,
) -> ProviderAccountFacts<P> {
    let exchange = AccountApiKeyExchange::new(AccountApiKeyResolver::new(api_http, hosts));
    facts
        .with_key_exchange(Arc::new(exchange.clone()))
        .with_team_key_resolver(Arc::new(exchange))
}

/// 账号事实源 + provider runtime headers 处理器。
/// 构造失败时回退到只读文件层（账号族 fail-closed），不影响 Agent 启动。
fn build_account_provider_source(
    paths: &ProviderPaths,
    data_root: &std::path::Path,
    app_version: &str,
) -> (
    Arc<dyn agent_process::ProviderSource>,
    Vec<agent_process::RequestHandler>,
    Option<AccountFactsRuntime>,
) {
    let first_http = match ReqwestCodingPlanHttp::new(Duration::from_secs(15)) {
        Ok(http) => http,
        Err(error) => {
            crate::host::shell_log(&format!(
                "coding-plan http unavailable, falling back to file layer: {error}"
            ));
            return (
                Arc::new(AgentProviderSource::local(
                    paths.bundled.clone(),
                    None,
                    paths.personal.clone(),
                )),
                Vec::new(),
                None,
            );
        }
    };
    let second_http = match ReqwestCodingPlanHttp::new(Duration::from_secs(15)) {
        Ok(http) => http,
        Err(error) => {
            crate::host::shell_log(&format!(
                "coding-plan http unavailable for runtime headers: {error}"
            ));
            return (
                Arc::new(AgentProviderSource::local(
                    paths.bundled.clone(),
                    None,
                    paths.personal.clone(),
                )),
                Vec::new(),
                None,
            );
        }
    };
    let settings_paths =
        settings_store::resolve_settings_paths(&settings_store::HomeEnv::from_process());
    let settings = Arc::new(settings_store::SettingsService::new(
        settings_paths,
        data_root.to_path_buf(),
    ));
    let credentials = credential_store::CredentialStore::new(data_root);
    let env_urls = EnvCodingPlanUrls::from_process(app_version);
    let hosts = AccountApiHosts {
        zai: env_urls.zai_business_base().to_string(),
        bigmodel: env_urls.bigmodel_origin().to_string(),
    };
    let mode_api_origin = env_urls.mode_origin().to_string();
    let urls: Arc<dyn provider_account_facts::CodingPlanUrlPort> = Arc::new(env_urls);
    let now: Arc<dyn Fn() -> f64 + Send + Sync> = Arc::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs_f64())
            .unwrap_or(0.0)
    });

    let source_facts = ProviderAccountFacts::new(
        first_http,
        Arc::new(ShellSettingsPort {
            settings: Arc::clone(&settings),
        }),
        Arc::new(ShellCredentialPort {
            store: credentials.clone(),
        }),
        Arc::clone(&urls),
        Arc::clone(&now),
    );
    let handler_facts = ProviderAccountFacts::new(
        second_http,
        Arc::new(ShellSettingsPort {
            settings: Arc::clone(&settings),
        }),
        Arc::new(ShellCredentialPort {
            store: credentials.clone(),
        }),
        Arc::clone(&urls),
        Arc::clone(&now),
    );
    let (source_facts, handler_facts) = match ReqwestAccountApiHttp::new(Duration::from_secs(15)) {
        Ok(api_http) => (
            attach_key_exchanges(source_facts, api_http.clone(), hosts.clone()),
            attach_key_exchanges(handler_facts, api_http, hosts),
        ),
        Err(error) => {
            crate::host::shell_log(&format!(
                "coding-plan key exchange unavailable: {error}"
            ));
            (source_facts, handler_facts)
        }
    };
    let handler = agent_process::provider_runtime_headers_handler(
        Arc::new(ShellRuntimeHeadersResolver {
            facts: Arc::new(handler_facts),
        }),
        tauri::async_runtime::handle().inner().clone(),
    );
    let official_handler = agent_process::official_mcp_auth_headers_handler(
        Arc::new(ShellOfficialMcpAuthResolver {
            settings: Arc::clone(&settings),
            credentials: credentials.clone(),
            mode_api_origin,
        }),
        tauri::async_runtime::handle().inner().clone(),
    );
    let provider_source = Arc::new(AgentProviderSource::new(
        paths.bundled.clone(),
        None,
        paths.personal.clone(),
        source_facts,
    ));
    let runtime = AccountFactsRuntime {
        source: Arc::clone(&provider_source),
        personal_path: paths.personal.clone(),
    };
    (
        provider_source as Arc<dyn agent_process::ProviderSource>,
        vec![handler, official_handler],
        Some(runtime),
    )
}

/// 装配 registry + 只读控制面（S0）。工厂完整实现启动规格解析，
/// 但 S0 阶段只有 `agent-health` 消费它（只读、不 spawn）。
pub fn build_control_client(config: &AgentRuntimeConfig) -> Arc<AgentControlClient> {
    let resources_dir = config.resources_dir.clone();
    let data_root = config.data_root.clone();
    let cwd = std::env::current_dir().unwrap_or_else(|_| data_root.clone());
    let provider_paths = resolve_provider_paths(
        resources_dir.as_deref(),
        &data_root,
        &cwd,
        &std::env::vars().collect(),
    );
    if provider_paths.is_none() {
        crate::host::shell_log(
            "provider source disabled: bundled mode-builtin.json not found",
        );
    }
    let provider_env = provider_paths.as_ref().map(|paths| {
        create_node_provider_runtime_path_env(&ProviderRuntimePaths {
            mode_builtin_file_path: paths.bundled.clone(),
            personal_file_path: paths.personal.to_string_lossy().into_owned(),
        })
    });
    // TS 在每次 Agent spawn 前调用 `prepareRuntimeState`（内置 Subagent 旧覆盖导入）。
    // 壳侧只在第一次 spawn 前执行一次；迁移是幂等的，避免无 Agent 时创建用户目录。
    let subagents = Arc::new(SubagentsService::new(SubagentsServiceOptions::from_process()));
    let runtime_state_prepared = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let data_root_for_source = data_root.clone();
    let factory: SpecFactory = Arc::new(move |target| {
        if !runtime_state_prepared.swap(true, std::sync::atomic::Ordering::SeqCst) {
            subagents.prepare_runtime_state();
        }
        let workspace_path = PathBuf::from(&target.workspace_path);
        let mut env: BTreeMap<String, String> = std::env::vars().collect();
        if let Some(extra) = &provider_env {
            env.extend(extra.clone());
        }
        let env_map: EnvMap = env.clone().into_iter().collect();
        let cwd = std::env::current_dir().unwrap_or_else(|_| data_root.clone());
        let home = home_dir();
        let native_binary = find_mode_agent_runtime_binary(&AgentRuntimeContext {
            platform: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            cwd: cwd.clone(),
            module_dir: None,
            resources_path: resources_dir
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            home_dir: home,
            env: env_map,
            fs: &NativeToolFileSystem,
        });
        let context = AgentResolveContext {
            workspace_path: workspace_path.clone(),
            env,
            // TS 的 findUpward 从宿主进程 cwd 起步；开发树在 cwd 之下才可见。
            upward_start: cwd,
            resources_dir: resources_dir.clone(),
            node_candidates: node_candidates(&resources_dir),
            node_fallback: "node".into(),
            deployed_native_binary: native_binary,
            presentation_surface: Some("desktop".into()),
            request_timeout: Duration::from_secs(180),
        };
        let resolved =
            resolve_agent_command(&context).map_err(|error| AgentProcessError::Spawn {
                command: String::new(),
                message: error.to_string(),
            })?;
        let mut spec = resolved.spec;
        // `resolve_agent_command` 不复制上下文 env（进程默认继承壳 env）；Provider 路径
        // 是壳内部解析出的，必须显式注入，否则 agent 侧读不到同一份配置。
        if let Some(extra) = &provider_env {
            for (key, value) in extra {
                spec.env.push((key.clone(), value.clone()));
            }
        }
        Ok(spec)
    });
    let control = Arc::new(AgentControlClient::new(Arc::new(
        AgentRuntimeRegistry::new(factory),
    )));
    if let Some(paths) = provider_paths {
            let (source, request_handlers, facts_runtime) = build_account_provider_source(
            &paths,
            &data_root_for_source,
            &config.app_version,
        );
        control.set_provider_source(Some(source));
        for handler in request_handlers {
            control.push_request_handler(handler);
        }
        if let Some(runtime) = facts_runtime {
            runtime.start(Arc::clone(&control));
        }
    }
    control
}

/// Personal Provider Config 变化 → Account Overlay 刷新 → 活跃 workspace 重交付。
///
/// 轮询由 `provider-config-repository` 的 1s 间隔承担；本结构只在 shell 生命周期内
/// 持有仓库与订阅，避免配置文件变化需要重启 Agent。
pub struct AccountFactsRuntime {
    source: Arc<AgentProviderSource>,
    personal_path: PathBuf,
}

impl AccountFactsRuntime {
    pub fn start(self, control: Arc<AgentControlClient>) {
        use provider_config_repository::{
            PersonalConfigRepository, PersonalConfigRepositoryOptions, PollingInterval,
        };

        let repository = match PersonalConfigRepository::new(PersonalConfigRepositoryOptions {
            file_path: self.personal_path.clone(),
            import_legacy: None,
            on_recovery: None,
            on_polling_error: None,
            polling_interval: PollingInterval::Millis(1_000),
        }) {
            Ok(repository) => repository,
            Err(error) => {
                crate::host::shell_log(&format!(
                    "personal provider config polling unavailable: {error}"
                ));
                return;
            }
        };
        let (change_tx, mut change_rx) =
            tokio::sync::mpsc::unbounded_channel::<String>();
        let subscription = repository.on_did_change(Arc::new(move |reason| {
            let _ = change_tx.send(reason.to_string());
        }));
        // 首次读取同时启动轮询线程。
        let _ = repository.read();
        let source = self.source;
        let control_for_refresh = control;
        crate::host::shell_log(&format!(
            "personal provider config polling started: {}",
            self.personal_path.display()
        ));
        tauri::async_runtime::spawn(async move {
            let _ = subscription;
            let _repository = repository;
            while let Some(reason) = change_rx.recv().await {
                // poll-error 只代表本轮读取失败，保留 LKG，不触发重交付。
                if reason == "poll-error" {
                    continue;
                }
                let refresh_reason = format!("personal:{reason}");
                if let Err(error) = source.refresh(&refresh_reason).await {
                    crate::host::shell_log(&format!(
                        "account overlay refresh failed: {error}"
                    ));
                    continue;
                }
                if let Err(error) = control_for_refresh.refresh_provider_config().await {
                    crate::host::shell_log(&format!(
                        "account overlay redelivery failed: {error}"
                    ));
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use provider_config_repository::{
        MODE_BUILTIN_PROVIDER_CONFIG_FILE_ENV, MODE_PERSONAL_PROVIDER_CONFIG_FILE_ENV,
    };

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mode-provider-paths-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_bundled(root: &Path) -> PathBuf {
        let dir = root.join("config").join("provider");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("mode-builtin.json");
        std::fs::write(&file, "{}").unwrap();
        file
    }

    #[test]
    fn explicit_env_paths_win() {
        let mut env = BTreeMap::new();
        env.insert(
            MODE_BUILTIN_PROVIDER_CONFIG_FILE_ENV.to_string(),
            "D:/builtin.json".into(),
        );
        env.insert(
            MODE_PERSONAL_PROVIDER_CONFIG_FILE_ENV.to_string(),
            "D:/personal.json".into(),
        );
        let paths = resolve_provider_paths(None, Path::new("D:/data"), Path::new("D:/cwd"), &env)
            .expect("env 路径");
        assert_eq!(paths.bundled, "D:/builtin.json");
        assert_eq!(paths.personal, PathBuf::from("D:/personal.json"));
    }

    #[test]
    fn resources_bundled_falls_back_to_data_root_personal() {
        let root = temp_dir("resources");
        let resources = root.join("resources");
        let bundled = write_bundled(&resources);
        let paths = resolve_provider_paths(
            Some(&resources),
            &root.join("data"),
            &root,
            &BTreeMap::new(),
        )
        .expect("resources 路径");
        assert_eq!(paths.bundled, bundled.to_string_lossy());
        assert_eq!(
            paths.personal,
            root.join("data").join("v2").join("provider_config.json")
        );
    }

    #[test]
    fn upward_search_finds_repo_bundled_config() {
        let root = temp_dir("upward");
        let bundled = write_bundled(&root);
        let cwd = root.join("apps").join("desktop-tauri");
        std::fs::create_dir_all(&cwd).unwrap();
        let paths =
            resolve_provider_paths(None, &root.join("data"), &cwd, &BTreeMap::new()).expect("upward");
        assert_eq!(paths.bundled, bundled.to_string_lossy());
    }

    #[test]
    fn missing_bundled_disables_source() {
        let root = temp_dir("missing");
        assert!(
            resolve_provider_paths(None, &root, &root, &BTreeMap::new()).is_none(),
            "找不到 Built-in 时不应注入 ProviderSource"
        );
    }
}
