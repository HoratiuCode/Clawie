use std::future::Future;
use std::pin::Pin;

use crate::error::ApiError;
use crate::types::{MessageRequest, MessageResponse};

pub mod anthropic;
pub mod openai_compat;

#[allow(dead_code)]
pub type ProviderFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, ApiError>> + Send + 'a>>;

#[allow(dead_code)]
pub trait Provider {
    type Stream;

    fn send_message<'a>(
        &'a self,
        request: &'a MessageRequest,
    ) -> ProviderFuture<'a, MessageResponse>;

    fn stream_message<'a>(
        &'a self,
        request: &'a MessageRequest,
    ) -> ProviderFuture<'a, Self::Stream>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Anthropic,
    Xai,
    OpenAi,
    Gemini,
    Kimi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderApi {
    Messages,
    ChatCompletions,
    Responses,
    ResponsesWebSocket,
}

impl ProviderApi {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Messages => "messages",
            Self::ChatCompletions => "chat_completions",
            Self::Responses => "responses",
            Self::ResponsesWebSocket => "responses_websocket",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderDefinition {
    pub kind: ProviderKind,
    pub name: &'static str,
    pub default_api: ProviderApi,
    pub apis: &'static [ProviderApi],
}

const ANTHROPIC_APIS: &[ProviderApi] = &[ProviderApi::Messages];
const OPENAI_COMPAT_APIS: &[ProviderApi] = &[
    ProviderApi::Responses,
    ProviderApi::ResponsesWebSocket,
    ProviderApi::ChatCompletions,
];

pub const ANTHROPIC_PROVIDER_DEFINITION: ProviderDefinition = ProviderDefinition {
    kind: ProviderKind::Anthropic,
    name: "Anthropic",
    default_api: ProviderApi::Messages,
    apis: ANTHROPIC_APIS,
};

pub const XAI_PROVIDER_DEFINITION: ProviderDefinition = ProviderDefinition {
    kind: ProviderKind::Xai,
    name: "xAI",
    default_api: ProviderApi::Responses,
    apis: OPENAI_COMPAT_APIS,
};

pub const OPENAI_PROVIDER_DEFINITION: ProviderDefinition = ProviderDefinition {
    kind: ProviderKind::OpenAi,
    name: "OpenAI",
    default_api: ProviderApi::Responses,
    apis: OPENAI_COMPAT_APIS,
};

pub const GEMINI_PROVIDER_DEFINITION: ProviderDefinition = ProviderDefinition {
    kind: ProviderKind::Gemini,
    name: "Gemini",
    default_api: ProviderApi::Responses,
    apis: OPENAI_COMPAT_APIS,
};

pub const KIMI_PROVIDER_DEFINITION: ProviderDefinition = ProviderDefinition {
    kind: ProviderKind::Kimi,
    name: "Kimi",
    default_api: ProviderApi::Responses,
    apis: OPENAI_COMPAT_APIS,
};

pub const PROVIDER_PREFERENCE_ENV: &str = "CLAW_PROVIDER";
pub const LEGACY_PROVIDER_PREFERENCE_ENV: &str = "CLAW_PROVIDER_PREFERENCE";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderMetadata {
    pub provider: ProviderKind,
    pub auth_env: &'static str,
    pub base_url_env: &'static str,
    pub default_base_url: &'static str,
}

const MODEL_REGISTRY: &[(&str, ProviderMetadata)] = &[
    (
        "opus",
        ProviderMetadata {
            provider: ProviderKind::Anthropic,
            auth_env: "ANTHROPIC_API_KEY",
            base_url_env: "ANTHROPIC_BASE_URL",
            default_base_url: anthropic::DEFAULT_BASE_URL,
        },
    ),
    (
        "sonnet",
        ProviderMetadata {
            provider: ProviderKind::Anthropic,
            auth_env: "ANTHROPIC_API_KEY",
            base_url_env: "ANTHROPIC_BASE_URL",
            default_base_url: anthropic::DEFAULT_BASE_URL,
        },
    ),
    (
        "haiku",
        ProviderMetadata {
            provider: ProviderKind::Anthropic,
            auth_env: "ANTHROPIC_API_KEY",
            base_url_env: "ANTHROPIC_BASE_URL",
            default_base_url: anthropic::DEFAULT_BASE_URL,
        },
    ),
    (
        "grok",
        ProviderMetadata {
            provider: ProviderKind::Xai,
            auth_env: "XAI_API_KEY",
            base_url_env: "XAI_BASE_URL",
            default_base_url: openai_compat::DEFAULT_XAI_BASE_URL,
        },
    ),
    (
        "grok-3",
        ProviderMetadata {
            provider: ProviderKind::Xai,
            auth_env: "XAI_API_KEY",
            base_url_env: "XAI_BASE_URL",
            default_base_url: openai_compat::DEFAULT_XAI_BASE_URL,
        },
    ),
    (
        "grok-mini",
        ProviderMetadata {
            provider: ProviderKind::Xai,
            auth_env: "XAI_API_KEY",
            base_url_env: "XAI_BASE_URL",
            default_base_url: openai_compat::DEFAULT_XAI_BASE_URL,
        },
    ),
    (
        "grok-3-mini",
        ProviderMetadata {
            provider: ProviderKind::Xai,
            auth_env: "XAI_API_KEY",
            base_url_env: "XAI_BASE_URL",
            default_base_url: openai_compat::DEFAULT_XAI_BASE_URL,
        },
    ),
    (
        "grok-2",
        ProviderMetadata {
            provider: ProviderKind::Xai,
            auth_env: "XAI_API_KEY",
            base_url_env: "XAI_BASE_URL",
            default_base_url: openai_compat::DEFAULT_XAI_BASE_URL,
        },
    ),
    (
        "gemini-1.5-pro",
        ProviderMetadata {
            provider: ProviderKind::Gemini,
            auth_env: "GEMINI_API_KEY",
            base_url_env: "GEMINI_BASE_URL",
            default_base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        },
    ),
    (
        "gemini-1.5-flash",
        ProviderMetadata {
            provider: ProviderKind::Gemini,
            auth_env: "GEMINI_API_KEY",
            base_url_env: "GEMINI_BASE_URL",
            default_base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        },
    ),
    (
        "gemini-2.0-pro",
        ProviderMetadata {
            provider: ProviderKind::Gemini,
            auth_env: "GEMINI_API_KEY",
            base_url_env: "GEMINI_BASE_URL",
            default_base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        },
    ),
    (
        "gemini-2.0-flash",
        ProviderMetadata {
            provider: ProviderKind::Gemini,
            auth_env: "GEMINI_API_KEY",
            base_url_env: "GEMINI_BASE_URL",
            default_base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        },
    ),
    (
        "gemini-3.5-flash",
        ProviderMetadata {
            provider: ProviderKind::Gemini,
            auth_env: "GEMINI_API_KEY",
            base_url_env: "GEMINI_BASE_URL",
            default_base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        },
    ),
    (
        "kimi",
        ProviderMetadata {
            provider: ProviderKind::Kimi,
            auth_env: "MOONSHOT_API_KEY",
            base_url_env: "MOONSHOT_BASE_URL",
            default_base_url: "https://api.moonshot.cn/v1",
        },
    ),
    (
        "moonshot-v1-auto",
        ProviderMetadata {
            provider: ProviderKind::Kimi,
            auth_env: "MOONSHOT_API_KEY",
            base_url_env: "MOONSHOT_BASE_URL",
            default_base_url: "https://api.moonshot.cn/v1",
        },
    ),
];

#[must_use]
pub fn resolve_model_alias(model: &str) -> String {
    let trimmed = model.trim();
    let lower = trimmed.to_ascii_lowercase();
    MODEL_REGISTRY
        .iter()
        .find_map(|(alias, metadata)| {
            (*alias == lower).then_some(match metadata.provider {
                ProviderKind::Anthropic => match *alias {
                    "opus" => "claude-opus-4-6",
                    "sonnet" => "claude-sonnet-4-6",
                    "haiku" => "claude-haiku-4-5-20251213",
                    _ => trimmed,
                },
                ProviderKind::Xai => match *alias {
                    "grok" | "grok-3" => "grok-3",
                    "grok-mini" | "grok-3-mini" => "grok-3-mini",
                    "grok-2" => "grok-2",
                    _ => trimmed,
                },
                ProviderKind::OpenAi => trimmed,
                ProviderKind::Gemini => trimmed,
                ProviderKind::Kimi => match *alias {
                    "kimi" => "moonshot-v1-auto",
                    _ => trimmed,
                },
            })
        })
        .map_or_else(|| trimmed.to_string(), ToOwned::to_owned)
}

#[must_use]
pub fn metadata_for_model(model: &str) -> Option<ProviderMetadata> {
    let canonical = resolve_model_alias(model);
    if canonical.starts_with("gpt-")
        || canonical.starts_with("openai/")
        || canonical.starts_with("anthropic/")
    {
        return Some(ProviderMetadata {
            provider: ProviderKind::OpenAi,
            auth_env: "OPENAI_API_KEY",
            base_url_env: "OPENAI_BASE_URL",
            default_base_url: openai_compat::DEFAULT_OPENAI_BASE_URL,
        });
    }
    if canonical.starts_with("claude") {
        return Some(ProviderMetadata {
            provider: ProviderKind::Anthropic,
            auth_env: "ANTHROPIC_API_KEY",
            base_url_env: "ANTHROPIC_BASE_URL",
            default_base_url: anthropic::DEFAULT_BASE_URL,
        });
    }
    if canonical.starts_with("grok") {
        return Some(ProviderMetadata {
            provider: ProviderKind::Xai,
            auth_env: "XAI_API_KEY",
            base_url_env: "XAI_BASE_URL",
            default_base_url: openai_compat::DEFAULT_XAI_BASE_URL,
        });
    }
    if canonical.starts_with("gemini-") {
        return Some(ProviderMetadata {
            provider: ProviderKind::Gemini,
            auth_env: "GEMINI_API_KEY",
            base_url_env: "GEMINI_BASE_URL",
            default_base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        });
    }
    None
}

#[must_use]
pub fn parse_provider_preference(value: &str) -> Option<ProviderKind> {
    match value.trim().to_ascii_lowercase().as_str() {
        "anthropic" | "claude" => Some(ProviderKind::Anthropic),
        "xai" | "grok" => Some(ProviderKind::Xai),
        "openai" | "gpt" => Some(ProviderKind::OpenAi),
        "gemini" | "google" => Some(ProviderKind::Gemini),
        _ => None,
    }
}

#[must_use]
pub fn provider_preference_from_env() -> Option<ProviderKind> {
    std::env::var(PROVIDER_PREFERENCE_ENV)
        .ok()
        .as_deref()
        .and_then(parse_provider_preference)
        .or_else(|| {
            std::env::var(LEGACY_PROVIDER_PREFERENCE_ENV)
                .ok()
                .as_deref()
                .and_then(parse_provider_preference)
        })
}

#[must_use]
pub const fn default_model_for_provider(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Anthropic => "claude-sonnet-4-6",
        ProviderKind::Xai => "grok-3",
        ProviderKind::OpenAi => "gpt-4.1",
        ProviderKind::Gemini => "gemini-1.5-pro",
        ProviderKind::Kimi => "moonshot-v1-auto",
    }
}

#[must_use]
pub const fn definition_for_provider(provider: ProviderKind) -> ProviderDefinition {
    match provider {
        ProviderKind::Anthropic => ANTHROPIC_PROVIDER_DEFINITION,
        ProviderKind::Xai => XAI_PROVIDER_DEFINITION,
        ProviderKind::OpenAi => OPENAI_PROVIDER_DEFINITION,
        ProviderKind::Gemini => GEMINI_PROVIDER_DEFINITION,
        ProviderKind::Kimi => KIMI_PROVIDER_DEFINITION,
    }
}

#[must_use]
pub fn provider_supports_api(provider: ProviderKind, api: ProviderApi) -> bool {
    definition_for_provider(provider)
        .apis
        .iter()
        .any(|candidate| *candidate == api)
}

#[must_use]
pub fn detect_provider_kind(model: &str) -> ProviderKind {
    if let Some(metadata) = metadata_for_model(model) {
        return metadata.provider;
    }
    if let Some(preferred) = provider_preference_from_env() {
        return preferred;
    }
    if openai_compat::has_api_key("GEMINI_API_KEY") || openai_compat::has_api_key("GOOGLE_API_KEY") {
        return ProviderKind::Gemini;
    }
    if openai_compat::has_api_key("OPENAI_API_KEY") {
        return ProviderKind::OpenAi;
    }
    if openai_compat::has_api_key("XAI_API_KEY") {
        return ProviderKind::Xai;
    }
    if anthropic::has_auth_from_env_or_saved().unwrap_or(false) {
        return ProviderKind::Anthropic;
    }
    ProviderKind::Anthropic
}

#[must_use]
pub fn definition_for_model(model: &str) -> ProviderDefinition {
    definition_for_provider(detect_provider_kind(model))
}

#[must_use]
pub fn max_tokens_for_model(model: &str) -> u32 {
    let canonical = resolve_model_alias(model);
    if canonical.starts_with("gpt-") || canonical.starts_with("openai/") {
        32_768
    } else if canonical.contains("opus") {
        32_000
    } else if canonical.starts_with("grok") {
        64_000
    } else if canonical.starts_with("gemini-") {
        65_536
    } else {
        64_000
    }
}

#[cfg(test)]
mod tests {
    use super::{
        default_model_for_provider, definition_for_model, definition_for_provider,
        detect_provider_kind, max_tokens_for_model, parse_provider_preference,
        provider_supports_api, resolve_model_alias, ProviderApi, ProviderKind,
    };

    #[test]
    fn resolves_grok_aliases() {
        assert_eq!(resolve_model_alias("grok"), "grok-3");
        assert_eq!(resolve_model_alias("grok-mini"), "grok-3-mini");
        assert_eq!(resolve_model_alias("grok-2"), "grok-2");
    }

    #[test]
    fn detects_provider_from_model_name_first() {
        assert_eq!(detect_provider_kind("grok"), ProviderKind::Xai);
        assert_eq!(
            detect_provider_kind("claude-sonnet-4-6"),
            ProviderKind::Anthropic
        );
    }

    #[test]
    fn parses_provider_preferences_and_defaults() {
        assert_eq!(
            parse_provider_preference("claude"),
            Some(ProviderKind::Anthropic)
        );
        assert_eq!(parse_provider_preference("gpt"), Some(ProviderKind::OpenAi));
        assert_eq!(parse_provider_preference("grok"), Some(ProviderKind::Xai));
        assert_eq!(
            default_model_for_provider(ProviderKind::Anthropic),
            "claude-sonnet-4-6"
        );
        assert_eq!(default_model_for_provider(ProviderKind::OpenAi), "gpt-4.1");
        assert_eq!(default_model_for_provider(ProviderKind::Xai), "grok-3");
    }

    #[test]
    fn exposes_provider_definitions_and_supported_apis() {
        let anthropic = definition_for_provider(ProviderKind::Anthropic);
        assert_eq!(anthropic.default_api, ProviderApi::Messages);
        assert!(provider_supports_api(
            ProviderKind::OpenAi,
            ProviderApi::ResponsesWebSocket
        ));
        assert!(!provider_supports_api(
            ProviderKind::Anthropic,
            ProviderApi::Responses
        ));

        let grok = definition_for_model("grok");
        assert_eq!(grok.kind, ProviderKind::Xai);
        assert_eq!(grok.default_api, ProviderApi::Responses);
    }

    #[test]
    fn keeps_existing_max_token_heuristic() {
        assert_eq!(max_tokens_for_model("opus"), 32_000);
        assert_eq!(max_tokens_for_model("grok-3"), 64_000);
        assert_eq!(max_tokens_for_model("gpt-4.1"), 32_768);
    }
}
