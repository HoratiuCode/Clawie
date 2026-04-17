use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::Command as ProcessCommand;

use crate::args::{OutputFormat, PermissionMode};
use crate::input::{LineEditor, ReadOutcome};
use crate::render::{Spinner, TerminalRenderer};
use runtime::{
    format_usd, glob_search, pricing_for_model, ConfigLoader, ConversationClient,
    ConversationMessage, ModelPricing, RuntimeError, StreamEvent, UsageAlertLevel, UsageSummary,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionConfig {
    pub model: String,
    pub permission_mode: PermissionMode,
    pub config: Option<PathBuf>,
    pub output_format: OutputFormat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionState {
    pub turns: usize,
    pub compacted_messages: usize,
    pub last_model: String,
    pub last_usage: UsageSummary,
}

impl SessionState {
    #[must_use]
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            turns: 0,
            compacted_messages: 0,
            last_model: model.into(),
            last_usage: UsageSummary::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandResult {
    Continue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand {
    Help,
    Status,
    Compact,
    Model { model: Option<String> },
    Permissions { mode: Option<String> },
    Config { section: Option<String> },
    Memory,
    Cost,
    Reload,
    Ps,
    Search { query: Option<String>, path: Option<String> },
    Move { source: Option<String>, destination: Option<String> },
    Clear { confirm: bool },
    Unknown(String),
}

impl SlashCommand {
    #[must_use]
    pub fn parse(input: &str) -> Option<Self> {
        let trimmed = input.trim();
        if !trimmed.starts_with('/') {
            return None;
        }

        let mut parts = trimmed.trim_start_matches('/').split_whitespace();
        let command = parts.next().unwrap_or_default();
        Some(match command {
            "help" => Self::Help,
            "status" => Self::Status,
            "compact" => Self::Compact,
            "model" => Self::Model {
                model: parts.next().map(ToOwned::to_owned),
            },
            "permissions" => Self::Permissions {
                mode: parts.next().map(ToOwned::to_owned),
            },
            "config" => Self::Config {
                section: parts.next().map(ToOwned::to_owned),
            },
            "memory" => Self::Memory,
            "cost" => Self::Cost,
            "reload" => Self::Reload,
            "ps" => Self::Ps,
            "search" => Self::Search {
                query: parts.next().map(ToOwned::to_owned),
                path: parts.next().map(ToOwned::to_owned),
            },
            "move" => Self::Move {
                source: parts.next().map(ToOwned::to_owned),
                destination: parts.next().map(ToOwned::to_owned),
            },
            "clear" => Self::Clear {
                confirm: parts.next() == Some("--confirm"),
            },
            other => Self::Unknown(other.to_string()),
        })
    }
}

struct SlashCommandHandler {
    command: SlashCommand,
    summary: &'static str,
}

const SLASH_COMMAND_HANDLERS: &[SlashCommandHandler] = &[
    SlashCommandHandler {
        command: SlashCommand::Help,
        summary: "Show command help",
    },
    SlashCommandHandler {
        command: SlashCommand::Status,
        summary: "Show current session status",
    },
    SlashCommandHandler {
        command: SlashCommand::Compact,
        summary: "Compact local session history",
    },
    SlashCommandHandler {
        command: SlashCommand::Model { model: None },
        summary: "Show or switch the active model",
    },
    SlashCommandHandler {
        command: SlashCommand::Permissions { mode: None },
        summary: "Show or switch the active permission mode",
    },
    SlashCommandHandler {
        command: SlashCommand::Config { section: None },
        summary: "Inspect current config path or section",
    },
    SlashCommandHandler {
        command: SlashCommand::Memory,
        summary: "Inspect loaded memory/instruction files",
    },
    SlashCommandHandler {
        command: SlashCommand::Cost,
        summary: "Show usage and estimated API cost",
    },
    SlashCommandHandler {
        command: SlashCommand::Reload,
        summary: "Reload config and refresh the model client",
    },
    SlashCommandHandler {
        command: SlashCommand::Ps,
        summary: "Show hidden session and workspace details",
    },
    SlashCommandHandler {
        command: SlashCommand::Search {
            query: None,
            path: None,
        },
        summary: "Search files or contents in the workspace",
    },
    SlashCommandHandler {
        command: SlashCommand::Move {
            source: None,
            destination: None,
        },
        summary: "Move a file or folder to another path",
    },
    SlashCommandHandler {
        command: SlashCommand::Clear { confirm: false },
        summary: "Start a fresh local session",
    },
];

pub struct CliApp {
    config: SessionConfig,
    renderer: TerminalRenderer,
    state: SessionState,
    conversation_client: ConversationClient,
    conversation_history: Vec<ConversationMessage>,
}

impl CliApp {
    pub fn new(config: SessionConfig) -> Result<Self, RuntimeError> {
        let state = SessionState::new(config.model.clone());
        let conversation_client = ConversationClient::from_env(config.model.clone())?;
        Ok(Self {
            config,
            renderer: TerminalRenderer::new(),
            state,
            conversation_client,
            conversation_history: Vec::new(),
        })
    }

    pub fn run_repl(&mut self) -> io::Result<()> {
        let mut editor = LineEditor::new("› ", Vec::new());
        println!("Rusty Claude CLI interactive mode");
        println!("Type /help for commands. Shift+Enter or Ctrl+J inserts a newline.");

        loop {
            match editor.read_line()? {
                ReadOutcome::Submit(input) => {
                    if input.trim().is_empty() {
                        continue;
                    }
                    self.handle_submission(&input, &mut io::stdout())?;
                }
                ReadOutcome::Cancel => continue,
                ReadOutcome::Exit => break,
            }
        }

        Ok(())
    }

    pub fn run_prompt(&mut self, prompt: &str, out: &mut impl Write) -> io::Result<()> {
        self.render_response(prompt, out)
    }

    pub fn handle_submission(
        &mut self,
        input: &str,
        out: &mut impl Write,
    ) -> io::Result<CommandResult> {
        if let Some(command) = SlashCommand::parse(input) {
            return self.dispatch_slash_command(command, out);
        }

        self.state.turns += 1;
        self.render_response(input, out)?;
        Ok(CommandResult::Continue)
    }

    fn dispatch_slash_command(
        &mut self,
        command: SlashCommand,
        out: &mut impl Write,
    ) -> io::Result<CommandResult> {
        match command {
            SlashCommand::Help => Self::handle_help(out),
            SlashCommand::Status => self.handle_status(out),
            SlashCommand::Compact => self.handle_compact(out),
            SlashCommand::Model { model } => self.handle_model(model.as_deref(), out),
            SlashCommand::Permissions { mode } => self.handle_permissions(mode.as_deref(), out),
            SlashCommand::Config { section } => self.handle_config(section.as_deref(), out),
            SlashCommand::Memory => self.handle_memory(out),
            SlashCommand::Cost => self.handle_cost(out),
            SlashCommand::Reload => self.handle_reload(out),
            SlashCommand::Ps => self.handle_ps(out),
            SlashCommand::Search { query, path } => self.handle_search(query.as_deref(), path.as_deref(), out),
            SlashCommand::Move { source, destination } => self.handle_move(source.as_deref(), destination.as_deref(), out),
            SlashCommand::Clear { confirm } => self.handle_clear(confirm, out),
            SlashCommand::Unknown(name) => {
                writeln!(out, "Unknown slash command: /{name}")?;
                Ok(CommandResult::Continue)
            }
        }
    }

    fn write_bullet(out: &mut impl Write, text: impl AsRef<str>) -> io::Result<()> {
        writeln!(out, "- {}", text.as_ref())
    }

    fn handle_help(out: &mut impl Write) -> io::Result<CommandResult> {
        writeln!(out, "Available commands")?;
        for handler in SLASH_COMMAND_HANDLERS {
            let name = match handler.command {
                SlashCommand::Help => "/help",
                SlashCommand::Status => "/status",
                SlashCommand::Compact => "/compact",
                SlashCommand::Model { .. } => "/model [model]",
                SlashCommand::Permissions { .. } => "/permissions [mode]",
                SlashCommand::Config { .. } => "/config [section]",
                SlashCommand::Memory => "/memory",
                SlashCommand::Cost => "/cost",
                SlashCommand::Reload => "/reload",
                SlashCommand::Ps => "/ps",
                SlashCommand::Search { .. } => "/search <query> [path]",
                SlashCommand::Move { .. } => "/move <source> <destination>",
                SlashCommand::Clear { .. } => "/clear [--confirm]",
                SlashCommand::Unknown(_) => continue,
            };
            Self::write_bullet(out, format!("`{name}` — {}", handler.summary))?;
        }
        Ok(CommandResult::Continue)
    }

    fn handle_status(&mut self, out: &mut impl Write) -> io::Result<CommandResult> {
        writeln!(out, "Status")?;
        Self::write_bullet(out, format!("turns: {}", self.state.turns))?;
        Self::write_bullet(out, format!("model: {}", self.state.last_model))?;
        Self::write_bullet(out, format!("permission mode: {:?}", self.config.permission_mode))?;
        Self::write_bullet(out, format!("output format: {:?}", self.config.output_format))?;
        Self::write_bullet(
            out,
            format!(
                "last usage: {} in / {} out",
                self.state.last_usage.input_tokens, self.state.last_usage.output_tokens
            ),
        )?;
        Self::write_bullet(
            out,
            format!(
                "config: {}",
                self.config
                    .config
                    .as_ref()
                    .map_or_else(|| String::from("<none>"), |path| path.display().to_string())
            ),
        )?;
        Ok(CommandResult::Continue)
    }

    fn handle_compact(&mut self, out: &mut impl Write) -> io::Result<CommandResult> {
        self.state.compacted_messages += self.state.turns;
        self.state.turns = 0;
        self.conversation_history.clear();
        writeln!(out, "Compact")?;
        Self::write_bullet(
            out,
            format!(
                "session history compacted: {} messages total",
                self.state.compacted_messages
            ),
        )?;
        Ok(CommandResult::Continue)
    }

    fn handle_model(
        &mut self,
        model: Option<&str>,
        out: &mut impl Write,
    ) -> io::Result<CommandResult> {
        match model {
            Some(model) => {
                self.config.model = model.to_string();
                self.state.last_model = model.to_string();
                self.conversation_client = ConversationClient::from_env(self.config.model.clone())
                    .map_err(|error| io::Error::other(error.to_string()))?;
                writeln!(out, "Model")?;
                Self::write_bullet(out, format!("active model set to {model}"))?;
            }
            None => {
                writeln!(out, "Model")?;
                Self::write_bullet(out, format!("active model: {}", self.config.model))?;
            }
        }
        Ok(CommandResult::Continue)
    }

    fn handle_permissions(
        &mut self,
        mode: Option<&str>,
        out: &mut impl Write,
    ) -> io::Result<CommandResult> {
        match mode {
            None => {
                writeln!(out, "Permission mode")?;
                Self::write_bullet(out, format!("{:?}", self.config.permission_mode))?;
            }
            Some("read-only") => {
                self.config.permission_mode = PermissionMode::ReadOnly;
                writeln!(out, "Permission mode")?;
                Self::write_bullet(out, "set to read-only")?;
            }
            Some("workspace-write") => {
                self.config.permission_mode = PermissionMode::WorkspaceWrite;
                writeln!(out, "Permission mode")?;
                Self::write_bullet(out, "set to workspace-write")?;
            }
            Some("danger-full-access") => {
                self.config.permission_mode = PermissionMode::DangerFullAccess;
                writeln!(out, "Permission mode")?;
                Self::write_bullet(out, "set to danger-full-access")?;
            }
            Some(other) => {
                writeln!(out, "Permission mode")?;
                Self::write_bullet(out, format!("unknown mode: {other}"))?;
            }
        }
        Ok(CommandResult::Continue)
    }

    fn handle_config(
        &mut self,
        section: Option<&str>,
        out: &mut impl Write,
    ) -> io::Result<CommandResult> {
        match section {
            None => {
                writeln!(out, "Config")?;
                Self::write_bullet(
                    out,
                    format!(
                        "path: {}",
                        self.config
                            .config
                            .as_ref()
                            .map_or_else(|| String::from("<none>"), |path| path.display().to_string())
                    ),
                )?;
            }
            Some(section) => {
                writeln!(out, "Config")?;
                Self::write_bullet(
                    out,
                    format!("section `{section}` is not fully implemented yet"),
                )?;
                Self::write_bullet(
                    out,
                    format!(
                        "current path: {}",
                        self.config
                            .config
                            .as_ref()
                            .map_or_else(|| String::from("<none>"), |path| path.display().to_string())
                    ),
                )?;
            }
        }
        Ok(CommandResult::Continue)
    }

    fn handle_memory(&mut self, out: &mut impl Write) -> io::Result<CommandResult> {
        writeln!(out, "Memory")?;
        Self::write_bullet(
            out,
            format!(
                "loaded config file: {}",
                self.config
                    .config
                    .as_ref()
                    .map_or_else(|| String::from("<none>"), |path| path.display().to_string())
            ),
        )?;
        Ok(CommandResult::Continue)
    }

    fn handle_cost(&mut self, out: &mut impl Write) -> io::Result<CommandResult> {
        let pricing = pricing_for_model(&self.state.last_model);
        let cost = pricing.map_or_else(
            || self.state.last_usage.estimate_cost_usd(),
            |pricing| self.state.last_usage.estimate_cost_usd_with_pricing(pricing),
        );
        writeln!(out, "Cost")?;
        Self::write_bullet(
            out,
            format!(
                "usage: {} input / {} output / {} cache-write / {} cache-read",
                self.state.last_usage.input_tokens,
                self.state.last_usage.output_tokens,
                self.state.last_usage.cache_creation_input_tokens,
                self.state.last_usage.cache_read_input_tokens
            ),
        )?;
        Self::write_bullet(
            out,
            format!(
                "estimated API cost for model {}: {}",
                self.state.last_model,
                format_usd(cost.total_cost_usd())
            ),
        )?;
        self.render_cost_alert_line(cost.total_cost_usd(), out)?;
        Ok(CommandResult::Continue)
    }

    fn handle_reload(&mut self, out: &mut impl Write) -> io::Result<CommandResult> {
        let cwd = env::current_dir().map_err(io::Error::other)?;
        let loaded = ConfigLoader::default_for(&cwd)
            .load()
            .map_err(|error| io::Error::other(error.to_string()))?;
        if let Some(model) = loaded.model() {
            self.config.model = model.to_string();
            self.state.last_model = model.to_string();
        }
        if let Some(config_model) = loaded.model() {
            self.conversation_client = ConversationClient::from_env(config_model.to_string())
                .map_err(|error| io::Error::other(error.to_string()))?;
        } else {
            self.conversation_client =
                ConversationClient::from_env(self.config.model.clone()).map_err(io::Error::other)?;
        }
        writeln!(out, "Reload")?;
        Self::write_bullet(out, "config reloaded and model client refreshed")?;
        Self::write_bullet(out, format!("active model: {}", self.config.model))?;
        Ok(CommandResult::Continue)
    }

    fn handle_ps(&mut self, out: &mut impl Write) -> io::Result<CommandResult> {
        let cwd = env::current_dir().map_err(io::Error::other)?;
        let branch = ProcessCommand::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(&cwd)
            .output()
            .ok()
            .and_then(|output| output.status.success().then_some(output.stdout))
            .and_then(|stdout| String::from_utf8(stdout).ok())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| String::from("<unknown>"));
        let session_path = cwd.join(".claw/sessions");
        let auto_save = self
            .config
            .config
            .as_ref()
            .map_or_else(|| String::from("<none>"), |path| path.display().to_string());
        writeln!(out, "Session")?;
        Self::write_bullet(out, format!("model: {}", self.state.last_model))?;
        Self::write_bullet(out, format!("permissions: {}", self.config.permission_mode.as_str()))?;
        Self::write_bullet(out, format!("branch: {}", branch))?;
        Self::write_bullet(out, format!("workspace: {}", cwd.display()))?;
        Self::write_bullet(out, format!("directory: {}", cwd.display()))?;
        Self::write_bullet(out, format!("session: {}", session_path.display()))?;
        Self::write_bullet(out, format!("auto-save: {}", auto_save))?;
        Ok(CommandResult::Continue)
    }

    fn handle_search(
        &mut self,
        query: Option<&str>,
        path: Option<&str>,
        out: &mut impl Write,
    ) -> io::Result<CommandResult> {
        let Some(query) = query.filter(|value| !value.trim().is_empty()) else {
            writeln!(out, "Search")?;
            Self::write_bullet(out, "usage: /search <query> [path]")?;
            return Ok(CommandResult::Continue);
        };
        let results = glob_search(query, path).map_err(|error| io::Error::other(error.to_string()))?;
        writeln!(out, "Search")?;
        Self::write_bullet(out, format!("results for `{query}`"))?;
        for entry in results.filenames.iter().take(20) {
            Self::write_bullet(out, entry.display().to_string())?;
        }
        if results.filenames.is_empty() {
            Self::write_bullet(out, "no matches found")?;
        }
        Ok(CommandResult::Continue)
    }

    fn handle_move(
        &mut self,
        source: Option<&str>,
        destination: Option<&str>,
        out: &mut impl Write,
    ) -> io::Result<CommandResult> {
        let (Some(source), Some(destination)) = (source, destination) else {
            writeln!(out, "Move")?;
            Self::write_bullet(out, "usage: /move <source> <destination>")?;
            return Ok(CommandResult::Continue);
        };
        let destination_path = PathBuf::from(destination);
        if let Some(parent) = destination_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(source, &destination_path)?;
        writeln!(out, "Move")?;
        Self::write_bullet(
            out,
            format!("moved `{source}` to `{}`", destination_path.display()),
        )?;
        Ok(CommandResult::Continue)
    }

    fn handle_clear(&mut self, confirm: bool, out: &mut impl Write) -> io::Result<CommandResult> {
        if !confirm {
            writeln!(out, "Clear")?;
            Self::write_bullet(out, "refusing to clear without confirmation")?;
            Self::write_bullet(out, "re-run as /clear --confirm")?;
            return Ok(CommandResult::Continue);
        }

        self.state.turns = 0;
        self.state.compacted_messages = 0;
        self.state.last_usage = UsageSummary::default();
        self.conversation_history.clear();
        writeln!(out, "Clear")?;
        Self::write_bullet(out, "started a fresh local session")?;
        Ok(CommandResult::Continue)
    }

    fn handle_stream_event(
        renderer: &TerminalRenderer,
        event: StreamEvent,
        stream_spinner: &mut Spinner,
        tool_spinner: &mut Spinner,
        saw_text: &mut bool,
        assistant_text: &mut String,
        turn_usage: &mut UsageSummary,
        out: &mut impl Write,
    ) {
        match event {
            StreamEvent::TextDelta(delta) => {
                if !*saw_text {
                    let _ =
                        stream_spinner.finish("Streaming response", renderer.color_theme(), out);
                    *saw_text = true;
                }
                assistant_text.push_str(&delta);
            }
            StreamEvent::ToolCallStart { name, input } => {
                if !assistant_text.trim().is_empty() {
                    let rendered = renderer.vertical_markdown_to_ansi(assistant_text.trim());
                    let _ = writeln!(out, "{rendered}");
                    assistant_text.clear();
                }
                if *saw_text {
                    let _ = writeln!(out);
                }
                let _ = tool_spinner.tick(
                    &format!("Running tool `{name}` with {input}"),
                    renderer.color_theme(),
                    out,
                );
            }
            StreamEvent::ToolCallResult {
                name,
                output,
                is_error,
            } => {
                let label = if is_error {
                    format!("Tool `{name}` failed")
                } else {
                    format!("Tool `{name}` completed")
                };
                let _ = tool_spinner.finish(&label, renderer.color_theme(), out);
                let rendered_output = format!("### Tool `{name}`\n\n```text\n{output}\n```\n");
                let _ = renderer.stream_markdown(&rendered_output, out);
            }
            StreamEvent::Usage(usage) => {
                *turn_usage = usage;
            }
        }
    }

    fn write_turn_output(
        &self,
        summary: &runtime::TurnSummary,
        out: &mut impl Write,
    ) -> io::Result<()> {
        match self.config.output_format {
            OutputFormat::Text => {
                writeln!(
                    out,
                    "\nToken usage: {} input / {} output",
                    self.state.last_usage.input_tokens, self.state.last_usage.output_tokens
                )?;
            }
            OutputFormat::Json => {
                writeln!(
                    out,
                    "{}",
                    serde_json::json!({
                        "message": summary.assistant_text,
                        "usage": {
                            "input_tokens": self.state.last_usage.input_tokens,
                            "output_tokens": self.state.last_usage.output_tokens,
                        }
                    })
                )?;
            }
            OutputFormat::Ndjson => {
                writeln!(
                    out,
                    "{}",
                    serde_json::json!({
                        "type": "message",
                        "text": summary.assistant_text,
                        "usage": {
                            "input_tokens": self.state.last_usage.input_tokens,
                            "output_tokens": self.state.last_usage.output_tokens,
                        }
                    })
                )?;
            }
        }
        Ok(())
    }

    fn render_response(&mut self, input: &str, out: &mut impl Write) -> io::Result<()> {
        let mut stream_spinner = Spinner::new();
        stream_spinner.tick(
            "Opening conversation stream",
            self.renderer.color_theme(),
            out,
        )?;

        let mut turn_usage = UsageSummary::default();
        let mut tool_spinner = Spinner::new();
        let mut saw_text = false;
        let mut assistant_text = String::new();
        let renderer = &self.renderer;

        let result =
            self.conversation_client
                .run_turn(&mut self.conversation_history, input, |event| {
                    Self::handle_stream_event(
                        renderer,
                        event,
                        &mut stream_spinner,
                        &mut tool_spinner,
                        &mut saw_text,
                        &mut assistant_text,
                        &mut turn_usage,
                        out,
                    );
                });

        let summary = match result {
            Ok(summary) => summary,
            Err(error) => {
                stream_spinner.fail(
                    "Streaming response failed",
                    self.renderer.color_theme(),
                    out,
                )?;
                return Err(io::Error::other(error));
            }
        };
        self.state.last_usage = summary.usage.clone();
        if !assistant_text.trim().is_empty() {
            let rendered = self.renderer.vertical_markdown_to_ansi(assistant_text.trim());
            writeln!(out, "{rendered}")?;
            assistant_text.clear();
        }
        if saw_text {
            writeln!(out)?;
        } else {
            stream_spinner.finish("Streaming response", self.renderer.color_theme(), out)?;
        }

        self.write_turn_output(&summary, out)?;
        self.render_usage_alerts(out)?;
        let _ = turn_usage;
        Ok(())
    }

    fn render_usage_alerts(&self, out: &mut impl Write) -> io::Result<()> {
        let total_cost = self
            .state
            .last_usage
            .estimate_cost_usd_with_pricing(
                pricing_for_model(&self.state.last_model)
                    .unwrap_or_else(ModelPricing::default_sonnet_tier),
            )
            .total_cost_usd();
        let token_alert = env::var("CLAWIE_USAGE_ALERT_TOKENS")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(100_000);
        let yellow_alert = env::var("CLAWIE_USAGE_ALERT_YELLOW_USD")
            .ok()
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(5.0);
        let red_alert = env::var("CLAWIE_USAGE_ALERT_RED_USD")
            .ok()
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(10.0);
        if self.state.last_usage.total_tokens() >= token_alert {
            writeln!(
                out,
                "Alert: this session has used {} tokens, which is above the alert threshold of {}.",
                self.state.last_usage.total_tokens(),
                token_alert
            )?;
        }
        self.render_cost_alert_line_with_thresholds(total_cost, yellow_alert, red_alert, out)?;
        Ok(())
    }

    fn render_cost_alert_line(&self, total_cost: f64, out: &mut impl Write) -> io::Result<()> {
        self.render_cost_alert_line_with_thresholds(total_cost, 5.0, 10.0, out)
    }

    fn render_cost_alert_line_with_thresholds(
        &self,
        total_cost: f64,
        yellow_alert: f64,
        red_alert: f64,
        out: &mut impl Write,
    ) -> io::Result<()> {
        let level = if total_cost >= red_alert {
            UsageAlertLevel::Red
        } else if total_cost >= yellow_alert {
            UsageAlertLevel::Yellow
        } else {
            UsageAlertLevel::Green
        };
        match level {
            UsageAlertLevel::Green => {}
            UsageAlertLevel::Yellow => {
                writeln!(
                    out,
                    "Alert [yellow]: estimated API cost is {} for model {}, which is above the yellow threshold of {}.",
                    format_usd(total_cost),
                    self.state.last_model,
                    format_usd(yellow_alert)
                )?;
            }
            UsageAlertLevel::Red => {
                writeln!(
                    out,
                    "Alert [red]: estimated API cost is {} for model {}, which is above the red threshold of {}.",
                    format_usd(total_cost),
                    self.state.last_model,
                    format_usd(red_alert)
                )?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::args::{OutputFormat, PermissionMode};

    use super::{CommandResult, SessionConfig, SlashCommand};

    #[test]
    fn parses_required_slash_commands() {
        assert_eq!(SlashCommand::parse("/help"), Some(SlashCommand::Help));
        assert_eq!(SlashCommand::parse(" /status "), Some(SlashCommand::Status));
        assert_eq!(
            SlashCommand::parse("/compact now"),
            Some(SlashCommand::Compact)
        );
        assert_eq!(
            SlashCommand::parse("/model claude-sonnet"),
            Some(SlashCommand::Model {
                model: Some("claude-sonnet".into()),
            })
        );
        assert_eq!(
            SlashCommand::parse("/permissions workspace-write"),
            Some(SlashCommand::Permissions {
                mode: Some("workspace-write".into()),
            })
        );
        assert_eq!(
            SlashCommand::parse("/config hooks"),
            Some(SlashCommand::Config {
                section: Some("hooks".into()),
            })
        );
        assert_eq!(SlashCommand::parse("/memory"), Some(SlashCommand::Memory));
        assert_eq!(
            SlashCommand::parse("/clear --confirm"),
            Some(SlashCommand::Clear { confirm: true })
        );
    }

    #[test]
    fn help_output_lists_commands() {
        let mut out = Vec::new();
        let result = super::CliApp::handle_help(&mut out).expect("help succeeds");
        assert_eq!(result, CommandResult::Continue);
        let output = String::from_utf8_lossy(&out);
        assert!(output.contains("/help"));
        assert!(output.contains("/status"));
        assert!(output.contains("/compact"));
        assert!(output.contains("/model [model]"));
        assert!(output.contains("/permissions [mode]"));
        assert!(output.contains("/config [section]"));
        assert!(output.contains("/memory"));
        assert!(output.contains("/clear [--confirm]"));
    }

    #[test]
    fn session_state_tracks_config_values() {
        let config = SessionConfig {
            model: "claude".into(),
            permission_mode: PermissionMode::DangerFullAccess,
            config: Some(PathBuf::from("settings.toml")),
            output_format: OutputFormat::Text,
        };

        assert_eq!(config.model, "claude");
        assert_eq!(config.permission_mode, PermissionMode::DangerFullAccess);
        assert_eq!(config.config, Some(PathBuf::from("settings.toml")));
    }
}
