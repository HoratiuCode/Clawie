mod client;
mod error;
mod prompt_cache;
mod providers;
mod sse;
mod types;

pub use client::{
    anthropic, oauth_token_is_expired, openai, read_base_url, read_xai_base_url,
    resolve_saved_oauth_token, resolve_startup_auth_source, xai, MessageStream, OAuthTokenSet,
    ProviderClient, ProviderFacade, ProviderRequest, ProviderSelection,
};
pub use error::ApiError;
pub use prompt_cache::{
    CacheBreakEvent, PromptCache, PromptCacheConfig, PromptCachePaths, PromptCacheRecord,
    PromptCacheStats,
};
pub use providers::anthropic::{AnthropicClient, AnthropicClient as ApiClient, AuthSource};
pub use providers::openai_compat::{OpenAiCompatClient, OpenAiCompatConfig};
pub use providers::{
    default_model_for_provider, definition_for_model, definition_for_provider,
    detect_provider_kind, max_tokens_for_model, metadata_for_model, parse_provider_preference,
    provider_preference_from_env, provider_supports_api, resolve_model_alias, ProviderApi,
    ProviderDefinition, ProviderKind, ANTHROPIC_PROVIDER_DEFINITION,
    LEGACY_PROVIDER_PREFERENCE_ENV, OPENAI_PROVIDER_DEFINITION, PROVIDER_PREFERENCE_ENV,
    XAI_PROVIDER_DEFINITION,
};
pub use sse::{parse_frame, SseParser};
pub use types::{
    ContentBlockDelta, ContentBlockDeltaEvent, ContentBlockStartEvent, ContentBlockStopEvent,
    InputContentBlock, InputMessage, MessageDelta, MessageDeltaEvent, MessageRequest,
    MessageResponse, MessageStartEvent, MessageStopEvent, OutputContentBlock, StreamEvent,
    ToolChoice, ToolDefinition, ToolResultContentBlock, Usage,
};

pub use telemetry::{
    AnalyticsEvent, AnthropicRequestProfile, ClientIdentity, JsonlTelemetrySink,
    MemoryTelemetrySink, SessionTraceRecord, SessionTracer, TelemetryEvent, TelemetrySink,
    DEFAULT_ANTHROPIC_VERSION,
};
