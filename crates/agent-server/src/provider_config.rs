//! Provider 配置与凭据解析（LT-1 C1 · Task 4b）。
//!
//! Agent 进程内 provider 事实的三层合并：
//! 1. **Built-in**（`MODE_BUILTIN_PROVIDER_CONFIG_FILE` 指向的随包/开发树 Release）；
//! 2. **Personal**（`MODE_PERSONAL_PROVIDER_CONFIG_FILE` 指向的 `provider_config.json`，
//!    每次解析现读，不持有可变状态）；
//! 3. **Account Overlay**（宿主经 `provider/updateAccountConfig` 交付的账号快照，
//!    进程内存持有，按 revision 去重）。
//!
//! 合并本身不在这里实现：文件层走 [`provider_config_source::read_snapshot`]（与壳侧
//! `AgentProviderSource` 同一读取口径），三层叠加走 [`provider_resolver`] 的纯解析器
//! （Personal 覆盖 Built-in/Account 的既有规则与其一致）。本模块只负责协议信封的
//! 严格解析、revision 去重、回执构造，以及把选中 `providerId/modelId` 投影成
//! [`ResolvedProvider`]（端点 + 凭据 + 能力），供 Task 7 的 `session/create` 构造
//! `provider-http` 适配器。
//!
//! 安全边界：`Debug`/`Display` 与错误文案永不携带 API key（镜像
//! `OpenAiChatConfig` / `AnthropicMessagesConfig` 的掩码做法）；OpenAI Responses
//! 适配器属 C2 · Task 5，这里显式报不支持而不是静默走错协议。本模块不做任何网络
//! 请求、不做 env 级模型 URL 重写、不连官方平台。

use std::collections::BTreeMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use agent_protocol::provider_runtime_headers::ProviderAccountAccess;
use indexmap::IndexMap;
use model_selection::ModelSelection;
use provider_config::{
    parse_account_provider_config_map, AccountProviderState, AccountProviderStates, Field,
    JsonValue, ProviderAccess, ProviderApiType, ProviderConfigMap, ZhipuAccountAccess,
};
use provider_config_repository::{
    resolve_node_provider_runtime_paths, MODE_BUILTIN_PROVIDER_CONFIG_FILE_ENV,
    MODE_PERSONAL_PROVIDER_CONFIG_FILE_ENV,
};
use provider_config_source::{read_snapshot, ProviderConfigSnapshot};
use provider_http::{AnthropicMessagesConfig, OpenAiChatConfig};
use provider_resolver::ProviderConfigResolverInput;
use serde::Deserialize;
use serde_json::Value;

/// 宿主 → Agent 的账号 Provider 配置交付方法名（与 `agent-process` 的发送方常量同值）。
pub const PROVIDER_UPDATE_ACCOUNT_CONFIG_METHOD: &str = "provider/updateAccountConfig";

/// 模型端点的线上协议格式。C1 适配 OpenAI Chat Completions，C2 · T4 增加
/// Anthropic Messages；OpenAI Responses 保留枚举身份，仍显式报不支持（C2 · T5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiFormat {
    OpenAiChatCompletions,
    AnthropicMessages,
    OpenAiResponses,
}

impl ApiFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiChatCompletions => "openai-chat-completions",
            Self::AnthropicMessages => "anthropic-messages",
            Self::OpenAiResponses => "openai-responses",
        }
    }
}

impl fmt::Display for ApiFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// 解析失败的显式错误面。变体本身不含凭据；文案只带 provider/model 与缺失字段名。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderConfigError {
    /// `provider/updateAccountConfig` 参数不合法（JSON-RPC `-32602`）。
    InvalidParams { message: String },
    /// 文件层 / env 路径不可用（builtin 缺失、路径未注入等）。
    ConfigUnavailable { message: String },
    /// 模型选择里的 provider 不在合并结果中。
    ProviderNotFound { provider_id: String },
    /// provider 存在但没有选中的模型。
    ModelNotFound {
        provider_id: String,
        model_id: String,
    },
    /// 缺少端点字段（`api.type` / `api.baseUrl` / `access.apiKey`）。
    MissingEndpointField {
        provider_id: String,
        model_id: String,
        field: &'static str,
    },
    /// `api.type` 存在但当前没有对应适配器（或请求的映射方向不匹配）。
    UnsupportedApiFormat {
        provider_id: String,
        model_id: String,
        api_format: String,
    },
}

impl ProviderConfigError {
    fn invalid_params(message: impl Into<String>) -> Self {
        Self::InvalidParams {
            message: message.into(),
        }
    }

    /// JSON-RPC 错误码：参数不合法 `-32602`，其余（配置/路径不可用）按内部错误
    /// `-32603` 暴露；Task 7 的 session 方法沿用这一映射。
    pub fn code(&self) -> i64 {
        match self {
            Self::InvalidParams { .. } => -32602,
            _ => -32603,
        }
    }
}

impl fmt::Display for ProviderConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidParams { message } | Self::ConfigUnavailable { message } => {
                formatter.write_str(message)
            }
            Self::ProviderNotFound { provider_id } => {
                write!(formatter, "Provider 不存在: {provider_id}")
            }
            Self::ModelNotFound {
                provider_id,
                model_id,
            } => write!(
                formatter,
                "Provider {provider_id} 不存在模型 {model_id}"
            ),
            Self::MissingEndpointField {
                provider_id,
                model_id,
                field,
            } => write!(
                formatter,
                "Provider {provider_id} 模型 {model_id} 缺少 {field}，无法解析可用端点"
            ),
            Self::UnsupportedApiFormat {
                provider_id,
                model_id,
                api_format,
            } => write!(
                formatter,
                "Provider {provider_id} 模型 {model_id} 的 apiFormat {api_format} 暂不支持（当前适配 openai-chat-completions / anthropic-messages）"
            ),
        }
    }
}

impl std::error::Error for ProviderConfigError {}

/// 已解析的模型端点与凭据。`api_key` 只经 `Debug`/`Display` 的掩码路径可见。
#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedProvider {
    pub provider_id: String,
    pub model_id: String,
    pub base_url: String,
    pub api_key: String,
    pub api_format: ApiFormat,
    pub max_output_tokens: Option<u64>,
    pub context_window: Option<u64>,
    /// provider 配置 `api.headers`（保序）；构造 `OpenAiChatConfig` 的 `extra_headers`。
    pub headers: Vec<(String, String)>,
}

/// 解析的中间形态（Task 8 控制器裁决 4）：凭据来自进程内配置（`api-key`）还是
/// 需要向宿主要运行时头（账号型 `zhipu-account` 族，OAuth→API Key 兑换归宿主）。
///
/// `resolve()` 对 [`Resolution::Account`] 保持既有口径（缺 `access.apiKey` 显式
/// 报错）；账号钩子的发起方（`HubProviderSource`）经 [`ProviderConfigHub::
/// resolve_account_target`] 拿到宿主请求所需事实，拿到 key 后用
/// [`ProviderConfigHub::resolve_account_provider`] 拼回 [`ResolvedProvider`]。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    ApiKey(ResolvedProvider),
    Account(AccountRuntimeHeadersTarget),
}

/// 账号型 provider 的可执行事实（端点/能力 + 账号访问描述）。**不含** API key——
/// key 由宿主经 `interaction/requestProviderRuntimeHeaders` 的应答提供。
#[derive(Clone, PartialEq, Eq)]
pub struct AccountRuntimeHeadersTarget {
    pub provider_id: String,
    pub model_id: String,
    pub base_url: String,
    pub api_format: ApiFormat,
    pub max_output_tokens: Option<u64>,
    pub context_window: Option<u64>,
    /// provider 配置 `api.headers`（保序）。
    pub headers: Vec<(String, String)>,
    /// 反向请求 params 的 `accountAccess`（协议严格形状）。
    pub account_access: ProviderAccountAccess,
}

/// `Debug` 与 [`ResolvedProvider`] 同口径：头值可能是另一把凭据，只显示头名。
impl fmt::Debug for AccountRuntimeHeadersTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AccountRuntimeHeadersTarget")
            .field("provider_id", &self.provider_id)
            .field("model_id", &self.model_id)
            .field("base_url", &self.base_url)
            .field("api_format", &self.api_format)
            .field("max_output_tokens", &self.max_output_tokens)
            .field("context_window", &self.context_window)
            .field(
                "headers",
                &self
                    .headers
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>(),
            )
            .field("account_access", &self.account_access)
            .finish()
    }
}

impl ResolvedProvider {
    /// 映射到 OpenAI Chat Completions 适配器配置。
    ///
    /// `api_format` 不是 `openai-chat-completions` 时显式报不支持，不允许静默走错协议
    /// （Anthropic 走 [`Self::to_anthropic_messages_config`]）。
    pub fn to_openai_chat_config(&self) -> Result<OpenAiChatConfig, ProviderConfigError> {
        if self.api_format != ApiFormat::OpenAiChatCompletions {
            return Err(self.unsupported_format());
        }
        Ok(OpenAiChatConfig {
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            model: self.model_id.clone(),
            max_output_tokens: self.max_output_tokens,
            extra_headers: self.headers.clone(),
        })
    }

    /// 映射到 Anthropic Messages 适配器配置（C2 · Task 4）。
    ///
    /// `api_format` 不是 `anthropic-messages` 时显式报不支持。provider 配置没有
    /// `anthropic-version` 字段，版本头用适配器的公开文档默认值（`None`）。
    pub fn to_anthropic_messages_config(
        &self,
    ) -> Result<AnthropicMessagesConfig, ProviderConfigError> {
        if self.api_format != ApiFormat::AnthropicMessages {
            return Err(self.unsupported_format());
        }
        Ok(AnthropicMessagesConfig {
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            model: self.model_id.clone(),
            max_output_tokens: self.max_output_tokens,
            extra_headers: self.headers.clone(),
            anthropic_version: None,
        })
    }

    fn unsupported_format(&self) -> ProviderConfigError {
        ProviderConfigError::UnsupportedApiFormat {
            provider_id: self.provider_id.clone(),
            model_id: self.model_id.clone(),
            api_format: self.api_format.as_str().to_owned(),
        }
    }
}

/// `Debug` 与 `OpenAiChatConfig` 同口径：密钥恒为掩码，头值（可能是另一把凭据）
/// 只显示头名。
impl fmt::Debug for ResolvedProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedProvider")
            .field("provider_id", &self.provider_id)
            .field("model_id", &self.model_id)
            .field("base_url", &self.base_url)
            .field("api_key", &"<redacted>")
            .field("api_format", &self.api_format)
            .field("max_output_tokens", &self.max_output_tokens)
            .field("context_window", &self.context_window)
            .field(
                "headers",
                &self
                    .headers
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl fmt::Display for ResolvedProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "provider={} model={} base_url={} api_format={} api_key=<redacted>",
            self.provider_id, self.model_id, self.base_url, self.api_format
        )
    }
}

/// `provider/updateAccountConfig` 的信封。信封级 `deny_unknown_fields`（严格解析）；
/// `states` 按 TS 交付快照「缺省空表」。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateAccountConfigParams {
    revision: String,
    based_on_mode_builtin_revision: String,
    providers: IndexMap<String, JsonValue>,
    #[serde(default)]
    states: IndexMap<String, AccountProviderState>,
}

/// 已接受的账号快照（解析后的类型化形态；revision 在 [`HubState`] 去重位）。
#[derive(Clone)]
struct AccountSnapshot {
    based_on_mode_builtin_revision: String,
    providers: ProviderConfigMap,
    states: AccountProviderStates,
}

#[derive(Default)]
struct HubState {
    /// 最近一次**接受**（不含失败交付）的快照 revision，去重依据。
    account_revision: Option<String>,
    account: Option<AccountSnapshot>,
}

/// Provider 配置 hub：账号快照接收 + 文件层合并 + 模型端点解析。
pub struct ProviderConfigHub {
    builtin_path: Option<String>,
    personal_path: Option<PathBuf>,
    /// 路径解析失败的延迟错误（env 只给了一边等）；resolve 时显式暴露。
    path_error: Option<String>,
    state: Mutex<HubState>,
}

impl fmt::Debug for ProviderConfigHub {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 不打印快照内容：账号快照本身不含 API key，但 hub 是凭据解析入口，
        // Debug 面只暴露 revision 级事实。
        let account_revision = self
            .state
            .try_lock()
            .ok()
            .and_then(|state| state.account_revision.clone());
        formatter
            .debug_struct("ProviderConfigHub")
            .field("builtin_path", &self.builtin_path)
            .field("personal_path", &self.personal_path)
            .field("path_error", &self.path_error)
            .field("account_revision", &account_revision)
            .finish()
    }
}

impl Default for ProviderConfigHub {
    fn default() -> Self {
        Self::from_env()
    }
}

impl ProviderConfigHub {
    /// 生产构造：路径来自 shell 注入的 `MODE_*_PROVIDER_CONFIG_FILE`。
    ///
    /// 两个 env 都未注入 → 文件层不可用（resolve 显式报错，构造不失败）；
    /// 只注入一边 → 复用 `resolve_node_provider_runtime_paths` 的 both-or-none
    /// 规则，错误延迟到 resolve 时暴露。
    pub fn from_env() -> Self {
        let env: BTreeMap<String, String> = std::env::vars().collect();
        match resolve_node_provider_runtime_paths(&env) {
            Ok(Some(paths)) => Self {
                builtin_path: Some(paths.mode_builtin_file_path),
                personal_path: Some(PathBuf::from(paths.personal_file_path)),
                path_error: None,
                state: Mutex::new(HubState::default()),
            },
            Ok(None) => Self {
                builtin_path: None,
                personal_path: None,
                path_error: None,
                state: Mutex::new(HubState::default()),
            },
            Err(error) => Self {
                builtin_path: None,
                personal_path: None,
                path_error: Some(error.message().to_owned()),
                state: Mutex::new(HubState::default()),
            },
        }
    }

    /// 测试/嵌入构造：显式指定 Built-in 与 Personal 文件路径。
    pub fn with_paths(builtin_path: impl Into<String>, personal_path: impl Into<PathBuf>) -> Self {
        Self {
            builtin_path: Some(builtin_path.into()),
            personal_path: Some(personal_path.into()),
            path_error: None,
            state: Mutex::new(HubState::default()),
        }
    }

    /// 最近一次接受的账号快照 revision（诊断/对账用）。
    pub fn account_revision(&self) -> Option<String> {
        self.state
            .lock()
            .expect("provider config state poisoned")
            .account_revision
            .clone()
    }

    /// 解析结果代际（provider 缓存键的一部分；最终评审 I6）。
    ///
    /// 由三部分组成：账号快照 revision、builtin 文件内容指纹、personal 文件内容指纹。
    /// personal 配置轮换 key / base URL 后指纹变化，worker 在下一个 turn 重新解析
    /// provider，不再复用旧实例（凭据/端点随配置走）。
    pub fn resolve_generation(&self) -> String {
        let account = self.account_revision().unwrap_or_default();
        format!(
            "account={account};builtin={};personal={}",
            file_fingerprint(self.builtin_path.as_deref().map(Path::new)),
            file_fingerprint(self.personal_path.as_deref())
        )
    }

    /// 最近一次接受的快照所基于的 Built-in revision（宿主交付值；诊断/对账用）。
    pub fn account_based_on_mode_builtin_revision(&self) -> Option<String> {
        self.state
            .lock()
            .expect("provider config state poisoned")
            .account
            .as_ref()
            .map(|snapshot| snapshot.based_on_mode_builtin_revision.clone())
    }

    /// 处理 `provider/updateAccountConfig` 请求。
    ///
    /// 严格解析 `{revision, basedOnModeBuiltinRevision, providers, states}`；
    /// revision 与最近一次接受的快照相同 → `unchanged` 且**不替换**已存储快照；
    /// 解析失败 → `-32602` 且保留上一份快照（fail-closed）。回执
    /// `{receivedRevision, providerCount, status}` 的 `receivedRevision` 恒等于交付
    /// `revision`：发送方（`agent-process::ProviderConfigSync`）只在回执版本一致时
    /// 才标记已交付，不一致或报错都会在下轮重试。
    pub fn handle_account_config(&self, params: Value) -> Result<Value, ProviderConfigError> {
        let parsed: UpdateAccountConfigParams =
            serde_json::from_value(params).map_err(|error| {
                ProviderConfigError::invalid_params(format!(
                    "provider/updateAccountConfig 参数不合法: {error}"
                ))
            })?;

        let mut state = self.state.lock().expect("provider config state poisoned");
        if state.account_revision.as_deref() == Some(parsed.revision.as_str()) {
            let provider_count = state
                .account
                .as_ref()
                .map(|snapshot| snapshot.providers.values.len())
                .unwrap_or(0);
            return Ok(receipt(&parsed.revision, provider_count, "unchanged"));
        }

        // 账号层配置用 provider-config 的 Account 模式解析器（不复制解析逻辑）：
        // 只接受 `access`（zhipu-account）与 `builtinModelIds`。
        let providers =
            parse_account_provider_config_map(&JsonValue::Object(parsed.providers), &[]).map_err(
                |error| {
                    ProviderConfigError::invalid_params(format!(
                        "provider/updateAccountConfig providers 不合法: {error:?}"
                    ))
                },
            )?;
        let provider_count = providers.values.len();
        state.account = Some(AccountSnapshot {
            based_on_mode_builtin_revision: parsed.based_on_mode_builtin_revision,
            providers,
            states: parsed.states,
        });
        state.account_revision = Some(parsed.revision.clone());
        Ok(receipt(&parsed.revision, provider_count, "received"))
    }

    /// 解析一次模型选择为可用端点 + 凭据 + 能力。
    ///
    /// 每次现读文件层（Personal 变更由壳侧 1s 轮询重交付账号快照，Personal 本身
    /// 由 Agent 自己读，现读即最新）；账号层取最近一次接受的快照。合并与 Personal
    /// 优先规则全部走 `provider-resolver` 公开 API，与壳侧 `AgentProviderSource`
    /// 的事实链一致。账号型 provider（`zhipu-account` 族）没有进程内 key：保持
    /// 既有口径，按缺 `access.apiKey` 显式报错（宿主钩子见
    /// [`Self::resolve_account_target`]）。
    pub fn resolve(
        &self,
        selection: &ModelSelection,
    ) -> Result<ResolvedProvider, ProviderConfigError> {
        match self.resolve_inner(selection)? {
            Resolution::ApiKey(resolved) => Ok(resolved),
            Resolution::Account(_) => Err(missing_field(selection, "access.apiKey")),
        }
    }

    /// 账号型 provider 的宿主钩子事实（Task 8 控制器裁决 4）。
    ///
    /// 只有账号型 provider（`access.type == "zhipu-account"`，`accountType`/`mode`
    /// 齐备）才返回 [`AccountRuntimeHeadersTarget`]；`api-key` provider 走
    /// [`Self::resolve`]，这里按同一「缺 `access.apiKey`」口径显式报错（不发起
    /// 反向请求）。`entitled` 缺失按 `false`（保守；宿主可自行拒绝）。
    pub fn resolve_account_target(
        &self,
        selection: &ModelSelection,
    ) -> Result<AccountRuntimeHeadersTarget, ProviderConfigError> {
        match self.resolve_inner(selection)? {
            Resolution::ApiKey(_) => Err(missing_field(selection, "access.apiKey")),
            Resolution::Account(target) => Ok(target),
        }
    }

    /// 宿主回 `requestAuth.apiKey` 后把账号型事实拼回可执行端点。
    ///
    /// 返回的 `api_format` 由适配器选择出口（[`ResolvedProvider::to_openai_chat_config`]
    /// / [`ResolvedProvider::to_anthropic_messages_config`]）校验，本方法不做协议裁决。
    pub fn resolve_account_provider(
        &self,
        target: &AccountRuntimeHeadersTarget,
        api_key: String,
    ) -> Result<ResolvedProvider, ProviderConfigError> {
        Ok(ResolvedProvider {
            provider_id: target.provider_id.clone(),
            model_id: target.model_id.clone(),
            base_url: target.base_url.clone(),
            api_key,
            api_format: target.api_format,
            max_output_tokens: target.max_output_tokens,
            context_window: target.context_window,
            headers: target.headers.clone(),
        })
    }

    /// 合并 + 端点事实的公共段；凭据来源分叉成 [`Resolution`]。
    fn resolve_inner(&self, selection: &ModelSelection) -> Result<Resolution, ProviderConfigError> {
        let file_snapshot = self.read_file_snapshot()?;
        let account = self
            .state
            .lock()
            .expect("provider config state poisoned")
            .account
            .clone();
        let empty_providers = ProviderConfigMap::empty();
        let (account_providers, account_states) = match &account {
            Some(snapshot) => (&snapshot.providers, Some(&snapshot.states)),
            None => (&empty_providers, None),
        };
        let input = ProviderConfigResolverInput {
            mode_builtin_providers: &file_snapshot.mode_builtin_providers,
            mode_builtin_provider_templates: Some(&file_snapshot.mode_builtin_provider_templates),
            personal_providers: &file_snapshot.personal_providers,
            mode_builtin_model_rules: &file_snapshot.mode_builtin_model_rules,
            personal_models: &file_snapshot.personal_models,
            account_providers,
            account_states,
            personal_provider_order: Some(&file_snapshot.personal_provider_order),
        };
        let resolution = provider_resolver::ProviderConfigResolver.resolve(&input);

        let resolved = resolution
            .resolved_providers
            .iter()
            .find(|provider| provider.provider_id == selection.provider_id)
            .ok_or_else(|| ProviderConfigError::ProviderNotFound {
                provider_id: selection.provider_id.clone(),
            })?;
        let model = resolved
            .models
            .iter()
            .find(|model| model.model_id == selection.model_id)
            .ok_or_else(|| ProviderConfigError::ModelNotFound {
                provider_id: selection.provider_id.clone(),
                model_id: selection.model_id.clone(),
            })?;
        let config = &resolved.config;

        // 1. apiFormat 先于端点/凭据：格式不支持时后者无意义。
        //    `api.type` 缺失同样显式报错（可执行 provider 在 Complete 校验下必有 type）。
        let api_format = match &config.api {
            Field::Value(api) => match api.api_type {
                Field::Value(ProviderApiType::OpenAiChatCompletions) => {
                    ApiFormat::OpenAiChatCompletions
                }
                Field::Value(ProviderApiType::AnthropicMessages) => ApiFormat::AnthropicMessages,
                Field::Value(ProviderApiType::OpenAiResponses) => {
                    return Err(ProviderConfigError::UnsupportedApiFormat {
                        provider_id: selection.provider_id.clone(),
                        model_id: selection.model_id.clone(),
                        api_format: ProviderApiType::OpenAiResponses.as_str().to_owned(),
                    });
                }
                _ => {
                    return Err(missing_field(selection, "api.type"));
                }
            },
            _ => return Err(missing_field(selection, "api.type")),
        };

        // 2. base URL（Personal 模式不校验 URL 形态，这里只挡缺失/空白）。
        let base_url = config
            .api
            .value()
            .and_then(|api| api.base_url.value())
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| missing_field(selection, "api.baseUrl"))?;

        // 3. 模型能力（token 上限）：来自模型规则解析结果；没有规则 → None，不编数字。
        let context_window = capability_u64(
            model
                .config
                .properties
                .value()
                .and_then(|properties| properties.context_window.value())
                .copied(),
        );
        let max_output_tokens = capability_u64(
            model
                .config
                .option_specs
                .value()
                .and_then(|specs| specs.max_output_tokens.value())
                .and_then(|limit| limit.max.value())
                .copied(),
        );
        let headers = config
            .api
            .value()
            .and_then(|api| api.headers.value())
            .map(|headers| {
                headers
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect()
            })
            .unwrap_or_default();

        // 4. 凭据来源分叉：`api-key` 直接带 key；账号型（`zhipu-account` 族）没有
        //    进程内 key（OAuth→API Key 兑换归宿主），交 [`Resolution::Account`]
        //    由调用方决定「显式报错」还是「发起宿主钩子」。
        match config.access.value() {
            Some(ProviderAccess::ApiKey(access)) => {
                let api_key = access
                    .api_key
                    .value()
                    .map(|value| value.trim().to_owned())
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| missing_field(selection, "access.apiKey"))?;
                Ok(Resolution::ApiKey(ResolvedProvider {
                    provider_id: selection.provider_id.clone(),
                    model_id: selection.model_id.clone(),
                    base_url,
                    api_key,
                    api_format,
                    max_output_tokens,
                    context_window,
                    headers,
                }))
            }
            Some(ProviderAccess::ZhipuAccount(ZhipuAccountAccess {
                account_type,
                mode,
                entitled,
            })) => {
                let account_type = account_type
                    .value()
                    .ok_or_else(|| missing_field(selection, "access.accountType"))?;
                let mode = mode
                    .value()
                    .ok_or_else(|| missing_field(selection, "access.mode"))?;
                Ok(Resolution::Account(AccountRuntimeHeadersTarget {
                    provider_id: selection.provider_id.clone(),
                    model_id: selection.model_id.clone(),
                    base_url,
                    api_format,
                    max_output_tokens,
                    context_window,
                    headers,
                    account_access: ProviderAccountAccess {
                        access_type:
                            agent_protocol::provider_runtime_headers::ProviderRuntimeAccessType::ZhipuAccount,
                        account_type: match account_type {
                            provider_config::ZhipuAccountType::Zai => {
                                agent_protocol::provider_runtime_headers::ProviderRuntimeAccountType::Zai
                            }
                            provider_config::ZhipuAccountType::Bigmodel => {
                                agent_protocol::provider_runtime_headers::ProviderRuntimeAccountType::Bigmodel
                            }
                        },
                        mode: match mode {
                            provider_config::ZhipuAccountMode::StartPlan => {
                                agent_protocol::provider_runtime_headers::ProviderRuntimeAccountMode::StartPlan
                            }
                            provider_config::ZhipuAccountMode::IndividualCodingPlan => {
                                agent_protocol::provider_runtime_headers::ProviderRuntimeAccountMode::IndividualCodingPlan
                            }
                            provider_config::ZhipuAccountMode::TeamCodingPlan => {
                                agent_protocol::provider_runtime_headers::ProviderRuntimeAccountMode::TeamCodingPlan
                            }
                            provider_config::ZhipuAccountMode::OffPeak => {
                                agent_protocol::provider_runtime_headers::ProviderRuntimeAccountMode::OffPeak
                            }
                        },
                        entitled: entitled.value().copied().unwrap_or(false),
                    },
                }))
            }
            // access 缺失：与 `api-key` 缺 key 同一显式口径。
            None => Err(missing_field(selection, "access.apiKey")),
        }
    }

    fn read_file_snapshot(&self) -> Result<ProviderConfigSnapshot, ProviderConfigError> {
        if let Some(message) = &self.path_error {
            return Err(ProviderConfigError::ConfigUnavailable {
                message: message.clone(),
            });
        }
        let (Some(builtin), Some(personal)) = (&self.builtin_path, &self.personal_path) else {
            return Err(ProviderConfigError::ConfigUnavailable {
                message: format!(
                    "Provider 文件层不可用：缺少 {MODE_BUILTIN_PROVIDER_CONFIG_FILE_ENV} / {MODE_PERSONAL_PROVIDER_CONFIG_FILE_ENV}"
                ),
            });
        };
        read_snapshot(builtin, personal, None).map_err(|error| {
            ProviderConfigError::ConfigUnavailable {
                message: error.to_string(),
            }
        })
    }
}

fn missing_field(selection: &ModelSelection, field: &'static str) -> ProviderConfigError {
    ProviderConfigError::MissingEndpointField {
        provider_id: selection.provider_id.clone(),
        model_id: selection.model_id.clone(),
        field,
    }
}

/// token 上限：非负 i64 → u64；缺失或非法（负数）按 None 处理，不编默认值。
fn capability_u64(value: Option<i64>) -> Option<u64> {
    value.filter(|value| *value >= 0).map(|value| value as u64)
}

fn receipt(revision: &str, provider_count: usize, status: &str) -> Value {
    serde_json::json!({
        "receivedRevision": revision,
        "providerCount": provider_count,
        "status": status,
    })
}

/// 分派层错误映射：参数不合法 `-32602`；其余（配置/路径不可用、端点缺失、格式
/// 不支持）按内部错误 `-32603` 暴露，文案由 [`ProviderConfigError::Display`] 决定。
impl From<ProviderConfigError> for crate::server::ServerError {
    fn from(error: ProviderConfigError) -> Self {
        match &error {
            ProviderConfigError::InvalidParams { .. } => {
                crate::server::ServerError::invalid_params(error.to_string())
            }
            _ => crate::server::ServerError::internal(error.to_string()),
        }
    }
}

/// 文件内容指纹（长度 + `DefaultHasher`）。缺失/不可读时把错误 kind 计入，
/// 让「文件从不可读变为可读」也改变代际。哈希只用于进程内缓存键，不落库。
fn file_fingerprint(path: Option<&Path>) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    match path {
        Some(path) => {
            path.to_string_lossy().hash(&mut hasher);
            match std::fs::read(path) {
                Ok(bytes) => {
                    bytes.len().hash(&mut hasher);
                    bytes.hash(&mut hasher);
                }
                Err(error) => {
                    "missing".hash(&mut hasher);
                    format!("{:?}", error.kind()).hash(&mut hasher);
                }
            }
        }
        None => "unconfigured".hash(&mut hasher),
    }
    format!("{:016x}", hasher.finish())
}
