//! 已发布旧 Provider store（`mode.model-providers.v1`）的读取类型。
//!
//! 真源：`legacyModelProviderSerialized.ts` 中 `legacyModelProviderConfigSchema`。

use indexmap::IndexMap;
use provider_config::JsonValue;
use serde_json::Number;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modality {
    Text,
    Image,
    Video,
    Audio,
    Pdf,
}

impl Modality {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "text" => Some(Self::Text),
            "image" => Some(Self::Image),
            "video" => Some(Self::Video),
            "audio" => Some(Self::Audio),
            "pdf" => Some(Self::Pdf),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Image => "image",
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Pdf => "pdf",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiFormat {
    AnthropicMessages,
    OpenAiChatCompletions,
    OpenAiResponses,
}

impl ApiFormat {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "anthropic-messages" => Some(Self::AnthropicMessages),
            "openai-chat-completions" => Some(Self::OpenAiChatCompletions),
            "openai-responses" => Some(Self::OpenAiResponses),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::AnthropicMessages => "anthropic-messages",
            Self::OpenAiChatCompletions => "openai-chat-completions",
            Self::OpenAiResponses => "openai-responses",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportedFormat {
    Anthropic,
    OpenAi,
    Responses,
    Gemini,
}

impl SupportedFormat {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "anthropic" => Some(Self::Anthropic),
            "openai" => Some(Self::OpenAi),
            "responses" => Some(Self::Responses),
            "gemini" => Some(Self::Gemini),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::Responses => "responses",
            Self::Gemini => "gemini",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Anthropic,
    OpenAi,
    OpenAiCompatible,
}

impl ProviderKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "anthropic" => Some(Self::Anthropic),
            "openai" => Some(Self::OpenAi),
            "openai-compatible" => Some(Self::OpenAiCompatible),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::OpenAiCompatible => "openai-compatible",
        }
    }

    pub fn kind_api_format(self) -> ApiFormat {
        match self {
            Self::Anthropic => ApiFormat::AnthropicMessages,
            Self::OpenAi => ApiFormat::OpenAiResponses,
            Self::OpenAiCompatible => ApiFormat::OpenAiChatCompletions,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderSource {
    Builtin,
    ModelsDev,
    Custom,
    Workspace,
}

impl ProviderSource {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "builtin" => Some(Self::Builtin),
            "models-dev" => Some(Self::ModelsDev),
            "custom" => Some(Self::Custom),
            "workspace" => Some(Self::Workspace),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::ModelsDev => "models-dev",
            Self::Custom => "custom",
            Self::Workspace => "workspace",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemDisabledReason {
    CodingPlanNotAuthenticated,
    CodingPlanNotConnected,
    CodingPlanAuthFailed,
    CodingPlanNotEntitled,
    OauthProviderInactive,
}

impl SystemDisabledReason {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "coding_plan_not_authenticated" => Some(Self::CodingPlanNotAuthenticated),
            "coding_plan_not_connected" => Some(Self::CodingPlanNotConnected),
            "coding_plan_auth_failed" => Some(Self::CodingPlanAuthFailed),
            "coding_plan_not_entitled" => Some(Self::CodingPlanNotEntitled),
            "oauth_provider_inactive" => Some(Self::OauthProviderInactive),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::CodingPlanNotAuthenticated => "coding_plan_not_authenticated",
            Self::CodingPlanNotConnected => "coding_plan_not_connected",
            Self::CodingPlanAuthFailed => "coding_plan_auth_failed",
            Self::CodingPlanNotEntitled => "coding_plan_not_entitled",
            Self::OauthProviderInactive => "oauth_provider_inactive",
        }
    }
}

/// `legacyModelProviderEndpointsSchema`：三段默认空字符串，字段缺省即 `""`。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacyEndpoints {
    pub anthropic: String,
    pub openai: String,
    pub gemini: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LegacyModelProvider {
    pub id: String,
    pub name: String,
    pub enabled: Option<bool>,
    pub system_disabled_reason: Option<SystemDisabledReason>,
    pub endpoints: LegacyEndpoints,
    pub api_format: Option<ApiFormat>,
    pub source: Option<ProviderSource>,
    pub models_dev_provider_id: Option<String>,
    pub api_key_required: Option<bool>,
    pub headers: Option<IndexMap<String, String>>,
    pub logo_url: Option<String>,
    pub api_key: String,
    pub api_key_url: Option<String>,
    pub models: Vec<String>,
    pub model_display_names: Option<IndexMap<String, String>>,
    pub model_supported_formats: Option<IndexMap<String, Vec<SupportedFormat>>>,
    pub provider_mappings: Option<IndexMap<String, JsonValue>>,
    pub created_at: Number,
    pub updated_at: Number,
}
