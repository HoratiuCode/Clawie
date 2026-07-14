use crate::error::ApiError;
use crate::prompt_cache::{PromptCache, PromptCacheRecord, PromptCacheStats};
use crate::providers::anthropic::{self, AnthropicClient, AuthSource};
use crate::providers::openai_compat::{self, OpenAiCompatClient, OpenAiCompatConfig};
use crate::providers::{self, ProviderApi, ProviderDefinition, ProviderKind};
use crate::types::{MessageRequest, MessageResponse, StreamEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderSelection {
    pub kind: ProviderKind,
    pub api: ProviderApi,
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
        Self::from_model_with_selection_and_anthropic_auth(model, selection, anthropic_auth)
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
        let resolved_model = providers::resolve_model_alias(model);
        let detected_provider = providers::detect_provider_kind(&resolved_model);
        let definition = providers::definition_for_provider(selection.kind);
        if detected_provider != selection.kind
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
                client: match anthropic_auth {
                    Some(auth) => AnthropicClient::from_auth(auth),
                    None => AnthropicClient::from_env()?,
                },
                api: selection.api,
            }),
            ProviderKind::Xai => Ok(Self::Xai {
                client: OpenAiCompatClient::from_env(OpenAiCompatConfig::xai())?,
                api: selection.api,
            }),
            ProviderKind::OpenAi => Ok(Self::OpenAi {
                client: OpenAiCompatClient::from_env(OpenAiCompatConfig::openai())?,
                api: selection.api,
            }),
            ProviderKind::Gemini => Ok(Self::Gemini {
                client: OpenAiCompatClient::from_env(OpenAiCompatConfig::gemini())?,
                api: selection.api,
            }),
            ProviderKind::Kimi => Ok(Self::Kimi {
                client: OpenAiCompatClient::from_env(OpenAiCompatConfig::kimi())?,
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
            Self::Anthropic { api, .. } | Self::Xai { api, .. } | Self::OpenAi { api, .. } | Self::Gemini { api, .. } | Self::Kimi { api, .. } => *api,
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
            Self::Xai { .. } | Self::OpenAi { .. } | Self::Gemini { .. } | Self::Kimi { .. } => None,
        }
    }

    #[must_use]
    pub fn take_last_prompt_cache_record(&self) -> Option<PromptCacheRecord> {
        match self {
            Self::Anthropic { client, .. } => client.take_last_prompt_cache_record(),
            Self::Xai { .. } | Self::OpenAi { .. } | Self::Gemini { .. } | Self::Kimi { .. } => None,
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
            Self::Xai { client, .. } | Self::OpenAi { client, .. } | Self::Gemini { client, .. } | Self::Kimi { client, .. } => client
                .stream_message(request)
                .await
                .map(MessageStream::OpenAiCompat),
        }
    }
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
    oauth_token_is_expired, resolve_saved_oauth_token, resolve_startup_auth_source, OAuthTokenSet,
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
    use crate::providers::{detect_provider_kind, resolve_model_alias, ProviderApi, ProviderKind};
    use crate::ApiError;

    use super::{anthropic, openai, xai, ProviderClient};

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
