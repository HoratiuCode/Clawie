use crate::error::ApiError;
use crate::prompt_cache::{PromptCache, PromptCacheRecord, PromptCacheStats};
use crate::providers::anthropic::{self, AnthropicClient, AuthSource};
use crate::providers::openai_compat::{self, OpenAiCompatClient, OpenAiCompatConfig};
use crate::providers::{self, ProviderApi, ProviderDefinition, ProviderKind};
use crate::types::{MessageRequest, MessageResponse, StreamEvent};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderSelection {
    pub kind: ProviderKind,
    pub api: ProviderApi,
}

#[derive(Debug, Clone, Default)]
pub struct ProviderConnectionOptions {
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub headers: BTreeMap<String, String>,
    pub anthropic_auth: Option<AuthSource>,
    pub allow_provider_mismatch: bool,
    pub credential_provider: Option<&'static str>,
    pub credential_env_vars: Option<&'static [&'static str]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderFacade {
    definition: ProviderDefinition,
}

impl ProviderFacade {
    #[must_use]
    pub const fn new(definition: ProviderDefinition) -> Self {
        Self { definition }
    }

    #[must_use]
    pub const fn definition(self) -> ProviderDefinition {
        self.definition
    }

    #[must_use]
    pub const fn model(self, model: &str) -> ProviderRequest<'_> {
        ProviderRequest::new(model, self.definition.kind, self.definition.default_api)
    }

    #[must_use]
    pub const fn messages(self, model: &str) -> ProviderRequest<'_> {
        ProviderRequest::new(model, self.definition.kind, ProviderApi::Messages)
    }

    #[must_use]
    pub const fn chat(self, model: &str) -> ProviderRequest<'_> {
        ProviderRequest::new(model, self.definition.kind, ProviderApi::ChatCompletions)
    }

    #[must_use]
    pub const fn responses(self, model: &str) -> ProviderRequest<'_> {
        ProviderRequest::new(model, self.definition.kind, ProviderApi::Responses)
    }

    #[must_use]
    pub const fn responses_websocket(self, model: &str) -> ProviderRequest<'_> {
        ProviderRequest::new(model, self.definition.kind, ProviderApi::ResponsesWebSocket)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderRequest<'a> {
    model: &'a str,
    provider: ProviderKind,
    api: ProviderApi,
}

impl<'a> ProviderRequest<'a> {
    #[must_use]
    pub const fn new(model: &'a str, provider: ProviderKind, api: ProviderApi) -> Self {
        Self {
            model,
            provider,
            api,
        }
    }

    #[must_use]
    pub const fn selection(self) -> ProviderSelection {
        ProviderSelection {
            kind: self.provider,
            api: self.api,
        }
    }

    pub fn client(self) -> Result<ProviderClient, ApiError> {
        ProviderClient::from_model_with_selection_and_anthropic_auth(
            self.model,
            self.selection(),
            None,
        )
    }

    pub fn client_with_anthropic_auth(
        self,
        anthropic_auth: Option<AuthSource>,
    ) -> Result<ProviderClient, ApiError> {
        ProviderClient::from_model_with_selection_and_anthropic_auth(
            self.model,
            self.selection(),
            anthropic_auth,
        )
    }
}

#[must_use]
pub const fn anthropic() -> ProviderFacade {
    ProviderFacade::new(providers::ANTHROPIC_PROVIDER_DEFINITION)
}

#[must_use]
pub const fn xai() -> ProviderFacade {
    ProviderFacade::new(providers::XAI_PROVIDER_DEFINITION)
}

#[must_use]
pub const fn openai() -> ProviderFacade {
    ProviderFacade::new(providers::OPENAI_PROVIDER_DEFINITION)
}

pub const fn gemini() -> ProviderFacade {
    ProviderFacade::new(providers::GEMINI_PROVIDER_DEFINITION)
}

#[must_use]
pub const fn kimi() -> ProviderFacade {
    ProviderFacade::new(providers::KIMI_PROVIDER_DEFINITION)
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum ProviderClient {
    Anthropic {
        client: AnthropicClient,
        api: ProviderApi,
    },
    Xai {
        client: OpenAiCompatClient,
        api: ProviderApi,
    },
    OpenAi {
        client: OpenAiCompatClient,
        api: ProviderApi,
    },
    Gemini {
        client: OpenAiCompatClient,
        api: ProviderApi,
    },
    Kimi {
        client: OpenAiCompatClient,
        api: ProviderApi,
    },
}

impl ProviderClient {
    pub fn from_model(model: &str) -> Result<Self, ApiError> {
        Self::from_model_with_anthropic_auth(model, None)
    }

    pub fn from_model_with_anthropic_auth(
        model: &str,
        anthropic_auth: Option<AuthSource>,
    ) -> Result<Self, ApiError> {
        let resolved_model = providers::resolve_model_alias(model);
        let definition = providers::definition_for_model(&resolved_model);
        let selection = ProviderSelection {
            kind: definition.kind,
            api: definition.default_api,
        };
        let metadata = providers::metadata_for_model(&resolved_model);
        let (credential_provider, credential_env_vars) =
            credential_override_for_metadata(metadata.as_ref());
        Self::from_model_with_selection_and_options(
            model,
            selection,
            ProviderConnectionOptions {
                base_url: metadata.map(|metadata| metadata.default_base_url.to_string()),
                anthropic_auth,
                credential_provider,
                credential_env_vars,
                ..ProviderConnectionOptions::default()
            },
        )
    }

    pub fn from_model_with_selection(
        model: &str,
        selection: ProviderSelection,
    ) -> Result<Self, ApiError> {
        Self::from_model_with_selection_and_anthropic_auth(model, selection, None)
    }

    pub fn from_model_with_selection_and_anthropic_auth(
        model: &str,
        selection: ProviderSelection,
        anthropic_auth: Option<AuthSource>,
    ) -> Result<Self, ApiError> {
        Self::from_model_with_selection_and_options(
            model,
            selection,
            ProviderConnectionOptions {
                anthropic_auth,
                ..ProviderConnectionOptions::default()
            },
        )
    }

    pub fn from_model_with_selection_and_options(
        model: &str,
        selection: ProviderSelection,
        options: ProviderConnectionOptions,
    ) -> Result<Self, ApiError> {
        let resolved_model = providers::resolve_model_alias(model);
        let detected_provider = providers::detect_provider_kind(&resolved_model);
        let definition = providers::definition_for_provider(selection.kind);
        if !options.allow_provider_mismatch
            && detected_provider != selection.kind
            && providers::metadata_for_model(&resolved_model).is_some()
        {
            return Err(ApiError::Auth(format!(
                "model {resolved_model} belongs to provider {:?}, not {:?}",
                detected_provider, selection.kind
            )));
        }
        if !providers::provider_supports_api(selection.kind, selection.api) {
            return Err(ApiError::UnsupportedProviderApi {
                provider: definition.name,
                api: selection.api.as_str(),
            });
        }
        match selection.kind {
            ProviderKind::Anthropic => Ok(Self::Anthropic {
                client: match options.api_key.as_deref() {
                    Some(api_key) => AnthropicClient::new(api_key),
                    None => match options.anthropic_auth.clone() {
                        Some(auth) => AnthropicClient::from_auth(auth),
                        None => AnthropicClient::from_env()?,
                    },
                }
                .with_base_url(
                    options
                        .base_url
                        .clone()
                        .unwrap_or_else(anthropic::read_base_url),
                )
                .with_headers(options.headers.clone()),
                api: selection.api,
            }),
            ProviderKind::Xai => Ok(Self::Xai {
                client: openai_compat_client(OpenAiCompatConfig::xai(), &options)?,
                api: selection.api,
            }),
            ProviderKind::OpenAi => Ok(Self::OpenAi {
                client: openai_compat_client(OpenAiCompatConfig::openai(), &options)?,
                api: selection.api,
            }),
            ProviderKind::Gemini => Ok(Self::Gemini {
                client: openai_compat_client(OpenAiCompatConfig::gemini(), &options)?,
                api: selection.api,
            }),
            ProviderKind::Kimi => Ok(Self::Kimi {
                client: openai_compat_client(OpenAiCompatConfig::kimi(), &options)?,
                api: selection.api,
            }),
        }
    }

    #[must_use]
    pub const fn provider_kind(&self) -> ProviderKind {
        match self {
            Self::Anthropic { .. } => ProviderKind::Anthropic,
            Self::Xai { .. } => ProviderKind::Xai,
            Self::OpenAi { .. } => ProviderKind::OpenAi,
            Self::Gemini { .. } => ProviderKind::Gemini,
            Self::Kimi { .. } => ProviderKind::Kimi,
        }
    }

    #[must_use]
    pub const fn provider_api(&self) -> ProviderApi {
        match self {
            Self::Anthropic { api, .. }
            | Self::Xai { api, .. }
            | Self::OpenAi { api, .. }
            | Self::Gemini { api, .. }
            | Self::Kimi { api, .. } => *api,
        }
    }

    #[must_use]
    pub const fn selection(&self) -> ProviderSelection {
        ProviderSelection {
            kind: self.provider_kind(),
            api: self.provider_api(),
        }
    }

    #[must_use]
    pub fn with_prompt_cache(self, prompt_cache: PromptCache) -> Self {
        match self {
            Self::Anthropic { client, api } => Self::Anthropic {
                client: client.with_prompt_cache(prompt_cache),
                api,
            },
            other => other,
        }
    }

    #[must_use]
    pub fn prompt_cache_stats(&self) -> Option<PromptCacheStats> {
        match self {
            Self::Anthropic { client, .. } => client.prompt_cache_stats(),
            Self::Xai { .. } | Self::OpenAi { .. } | Self::Gemini { .. } | Self::Kimi { .. } => {
                None
            }
        }
    }

    #[must_use]
    pub fn take_last_prompt_cache_record(&self) -> Option<PromptCacheRecord> {
        match self {
            Self::Anthropic { client, .. } => client.take_last_prompt_cache_record(),
            Self::Xai { .. } | Self::OpenAi { .. } | Self::Gemini { .. } | Self::Kimi { .. } => {
                None
            }
        }
    }

    pub async fn send_message(
        &self,
        request: &MessageRequest,
    ) -> Result<MessageResponse, ApiError> {
        match self {
            Self::Anthropic { client, .. } => client.send_message(request).await,
            Self::Xai { client, .. }
            | Self::OpenAi { client, .. }
            | Self::Gemini { client, .. }
            | Self::Kimi { client, .. } => client.send_message(request).await,
        }
    }

    pub async fn stream_message(
        &self,
        request: &MessageRequest,
    ) -> Result<MessageStream, ApiError> {
        match self {
            Self::Anthropic { client, .. } => client
                .stream_message(request)
                .await
                .map(MessageStream::Anthropic),
            Self::Xai { client, .. }
            | Self::OpenAi { client, .. }
            | Self::Gemini { client, .. }
            | Self::Kimi { client, .. } => client
                .stream_message(request)
                .await
                .map(MessageStream::OpenAiCompat),
        }
    }
}

fn credential_override_for_metadata(
    metadata: Option<&providers::ProviderMetadata>,
) -> (Option<&'static str>, Option<&'static [&'static str]>) {
    match metadata.map(|metadata| metadata.auth_env) {
        Some("DASHSCOPE_API_KEY") => (Some("DashScope"), Some(&["DASHSCOPE_API_KEY"])),
        _ => (None, None),
    }
}

fn openai_compat_client(
    config: OpenAiCompatConfig,
    options: &ProviderConnectionOptions,
) -> Result<OpenAiCompatClient, ApiError> {
    let client = match &options.api_key {
        Some(api_key) => OpenAiCompatClient::new(api_key.clone(), config),
        None => match (options.credential_provider, options.credential_env_vars) {
            (Some(provider), Some(env_vars)) => {
                let api_key = env_vars
                    .iter()
                    .find_map(|env_var| std::env::var(env_var).ok())
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| ApiError::missing_credentials(provider, env_vars))?;
                OpenAiCompatClient::new(api_key, config)
            }
            _ => OpenAiCompatClient::from_env(config)?,
        },
    };
    Ok(match &options.base_url {
        Some(base_url) => client.with_base_url(base_url.clone()),
        None => client,
    }
    .with_headers(options.headers.clone()))
}

#[derive(Debug)]
pub enum MessageStream {
    Anthropic(anthropic::MessageStream),
    OpenAiCompat(openai_compat::MessageStream),
}

impl MessageStream {
    #[must_use]
    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::Anthropic(stream) => stream.request_id(),
            Self::OpenAiCompat(stream) => stream.request_id(),
        }
    }

    pub async fn next_event(&mut self) -> Result<Option<StreamEvent>, ApiError> {
        match self {
            Self::Anthropic(stream) => stream.next_event().await,
            Self::OpenAiCompat(stream) => stream.next_event().await,
        }
    }
}

pub use anthropic::{
    OAuthTokenSet, oauth_token_is_expired, resolve_saved_oauth_token, resolve_startup_auth_source,
};
#[must_use]
pub fn read_base_url() -> String {
    openai_compat::read_base_url(OpenAiCompatConfig::openai())
}

#[must_use]
pub fn read_xai_base_url() -> String {
    openai_compat::read_base_url(OpenAiCompatConfig::xai())
}

#[cfg(test)]
mod tests {
    use crate::ApiError;
    use crate::providers::{ProviderApi, ProviderKind, detect_provider_kind, resolve_model_alias};

    use super::{ProviderClient, anthropic, kimi, openai, xai};

    #[test]
    fn resolves_existing_and_grok_aliases() {
        assert_eq!(resolve_model_alias("opus"), "claude-opus-4-6");
        assert_eq!(resolve_model_alias("grok"), "grok-3");
        assert_eq!(resolve_model_alias("grok-mini"), "grok-3-mini");
    }

    #[test]
    fn provider_detection_prefers_model_family() {
        assert_eq!(detect_provider_kind("grok-3"), ProviderKind::Xai);
        assert_eq!(
            detect_provider_kind("claude-sonnet-4-6"),
            ProviderKind::Anthropic
        );
    }

    #[test]
    fn facades_expose_explicit_default_and_named_apis() {
        let openai_request = openai().model("gpt-4.1");
        assert_eq!(openai_request.selection().api, ProviderApi::Responses);

        let xai_request = xai().chat("grok-3");
        assert_eq!(xai_request.selection().api, ProviderApi::ChatCompletions);

        let anthropic_request = anthropic().messages("claude-sonnet-4-6");
        assert_eq!(anthropic_request.selection().api, ProviderApi::Messages);

        let kimi_request = kimi().responses("moonshot-v1-auto");
        assert_eq!(kimi_request.selection().api, ProviderApi::Responses);
    }

    #[test]
    fn rejects_unsupported_provider_api_pairings() {
        let error = ProviderClient::from_model_with_selection(
            "claude-sonnet-4-6",
            anthropic().responses("claude-sonnet-4-6").selection(),
        )
        .expect_err("anthropic responses route should be unsupported");
        assert!(matches!(error, ApiError::UnsupportedProviderApi { .. }));
    }
}
