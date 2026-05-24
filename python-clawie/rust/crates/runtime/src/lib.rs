mod bash;
mod bootstrap;
mod compact;
mod config;
mod conversation;
mod file_ops;
mod hooks;
mod json;
mod mcp;
mod mcp_client;
mod mcp_stdio;
mod oauth;
mod overflow;
mod permissions;
mod prompt;
mod remote;
mod revert;
pub mod sandbox;
mod session;
mod session_retry;
mod structured_output;
mod todo;
mod usage;

pub use bash::{execute_bash, BashCommandInput, BashCommandOutput};
pub use bootstrap::{BootstrapPhase, BootstrapPlan};
pub use compact::{
    compact_session, estimate_session_tokens, format_compact_summary,
    get_compact_continuation_message, should_compact, CompactionConfig, CompactionResult,
};
pub use config::{
    ConfigEntry, ConfigError, ConfigLoader, ConfigSource, McpConfigCollection,
    McpManagedProxyServerConfig, McpOAuthConfig, McpRemoteServerConfig, McpSdkServerConfig,
    McpServerConfig, McpStdioServerConfig, McpTransport, McpWebSocketServerConfig, OAuthConfig,
    ResolvedPermissionMode, RuntimeConfig, RuntimeFeatureConfig, RuntimeHookConfig,
    RuntimePluginConfig, ScopedMcpServerConfig, CLAW_SETTINGS_SCHEMA_NAME,
};
pub use conversation::{
    ApiClient, ApiRequest, AssistantEvent, ConversationRuntime, RuntimeError, StaticToolExecutor,
    ToolError, ToolExecutor, TurnSummary,
};
pub use file_ops::{
    edit_file, glob_search, grep_search, read_file, write_file, EditFileOutput, GlobSearchOutput,
    GrepSearchInput, GrepSearchOutput, ReadFileOutput, StructuredPatchHunk, TextFilePayload,
    WriteFileOutput,
};
pub use hooks::{HookEvent, HookRunResult, HookRunner};
pub use lsp::{
    FileDiagnostics, LspContextEnrichment, LspError, LspManager, LspServerConfig, SymbolLocation,
    WorkspaceDiagnostics,
};
pub use mcp::{
    mcp_server_signature, mcp_tool_name, mcp_tool_prefix, normalize_name_for_mcp,
    scoped_mcp_config_hash, unwrap_ccr_proxy_url,
};
pub use mcp_client::{
    McpClientAuth, McpClientBootstrap, McpClientTransport, McpManagedProxyTransport,
    McpRemoteTransport, McpSdkTransport, McpStdioTransport,
};
pub use mcp_stdio::{
    spawn_mcp_stdio_process, JsonRpcError, JsonRpcId, JsonRpcRequest, JsonRpcResponse,
    ManagedMcpTool, McpInitializeClientInfo, McpInitializeParams, McpInitializeResult,
    McpInitializeServerInfo, McpListResourcesParams, McpListResourcesResult, McpListToolsParams,
    McpListToolsResult, McpReadResourceParams, McpReadResourceResult, McpResource,
    McpResourceContents, McpServerManager, McpServerManagerError, McpStdioProcess, McpTool,
    McpToolCallContent, McpToolCallParams, McpToolCallResult, UnsupportedMcpServer,
};
pub use oauth::{
    clear_oauth_credentials, code_challenge_s256, credentials_path, generate_pkce_pair,
    generate_state, load_oauth_credentials, loopback_redirect_uri, parse_oauth_callback_query,
    parse_oauth_callback_request_target, save_oauth_credentials, OAuthAuthorizationRequest,
    OAuthCallbackParams, OAuthRefreshRequest, OAuthTokenExchangeRequest, OAuthTokenSet,
    PkceChallengeMethod, PkceCodePair,
};
pub use overflow::{status as overflow_status, ContextWindow, OverflowStatus};
pub use permissions::{
    PermissionMode, PermissionOutcome, PermissionPolicy, PermissionPromptDecision,
    PermissionPrompter, PermissionRequest,
};
pub use prompt::{
    load_system_prompt, prepend_bullets, ContextFile, ProjectContext, PromptBuildError,
    SystemPromptBuilder, FRONTIER_MODEL_NAME, SYSTEM_PROMPT_DYNAMIC_BOUNDARY,
};
pub use remote::{
    inherited_upstream_proxy_env, no_proxy_list, read_token, upstream_proxy_ws_url,
    RemoteSessionContext, UpstreamProxyBootstrap, UpstreamProxyState, DEFAULT_REMOTE_BASE_URL,
    DEFAULT_SESSION_TOKEN_PATH, DEFAULT_SYSTEM_CA_BUNDLE, NO_PROXY_HOSTS, UPSTREAM_PROXY_ENV_KEYS,
};
pub use revert::{
    checkpoint as session_checkpoint, revert_to_checkpoint, revert_to_last_user_turn,
    truncate_after_tool_use, RevertResult, SessionCheckpoint,
};
pub use session::{ContentBlock, ConversationMessage, MessageRole, Session, SessionError};
pub use session_retry::{is_retryable_error, RetryDecision, RetryPolicy};
pub use structured_output::{extract_json_candidate, StructuredOutput, StructuredOutputSpec};
pub use todo::{validate_todos, SessionTodo, SessionTodoList, TodoStatus};
pub use usage::{
    format_usd, pricing_for_model, ModelPricing, TokenUsage, UsageCostEstimate, UsageTracker,
};

#[cfg(test)]
pub(crate) fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
