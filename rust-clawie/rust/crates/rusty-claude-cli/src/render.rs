use std::fmt::Write as FmtWrite;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crossterm::cursor::MoveToColumn;
use crossterm::style::{Color, Print, ResetColor, SetForegroundColor, Stylize};
use crossterm::terminal::{Clear, ClearType};
use crossterm::{execute, queue};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::{as_24_bit_terminal_escaped, LinesWithEndings};

static ACTIVE_TERMINAL_THEME: AtomicU8 = AtomicU8::new(TerminalTheme::Emoji as u8);
/// When true, the live spinner stops redrawing so reply text can own the line.
static SPINNER_HELD: AtomicBool = AtomicBool::new(false);
/// Serialize all terminal writes so spinner / tools / replies never interleave.
static STDOUT_LOCK: Mutex<()> = Mutex::new(());
/// Don't flash a spinner for turns that finish almost immediately.
pub const SPINNER_DELAY: Duration = Duration::from_millis(300);

/// Pause the thinking spinner so streamed / structured replies are not overwritten.
pub fn hold_spinner() {
    SPINNER_HELD.store(true, Ordering::Relaxed);
}

/// Allow the thinking spinner to redraw again (call at the start of each turn).
pub fn release_spinner() {
    SPINNER_HELD.store(false, Ordering::Relaxed);
}

#[must_use]
pub fn spinner_is_held() -> bool {
    SPINNER_HELD.load(Ordering::Relaxed)
}

/// Lock stdout for exclusive writing (spinner, tools, replies).
pub fn lock_stdout() -> MutexGuard<'static, ()> {
    STDOUT_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Ensure the terminal is not in raw mode and every line starts at column 0.
///
/// In raw mode, bare `\n` only moves the cursor down (same column) — that is what
/// produced the staircase:
/// ```text
///   ✦ clawie
///             • Hello!
///                       • How can I help?
/// ```
pub fn prepare_cooked_stdout() {
    let _ = crossterm::terminal::disable_raw_mode();
}

/// Write one left-aligned terminal line using CRLF (raw-mode safe).
pub fn write_term_line(out: &mut (impl Write + ?Sized), line: &str) -> io::Result<()> {
    out.write_all(line.as_bytes())?;
    out.write_all(b"\r\n")?;
    Ok(())
}

/// Write a blank terminal line (raw-mode safe).
pub fn write_term_blank(out: &mut (impl Write + ?Sized)) -> io::Result<()> {
    out.write_all(b"\r\n")?;
    Ok(())
}

/// Clear the current line and move to column 0 (for taking over the spinner line).
pub fn clear_current_line(out: &mut (impl Write + ?Sized)) -> io::Result<()> {
    out.write_all(b"\r\x1b[2K")?;
    Ok(())
}

/// Quiet turn footer: `✓ ready · 1.2s · 1.4k↓ 320↑`
#[must_use]
pub fn format_turn_footer(
    elapsed: Duration,
    input_tokens: u32,
    output_tokens: u32,
    failed: bool,
) -> String {
    let secs = elapsed.as_secs_f64();
    let time = if secs < 10.0 {
        format!("{secs:.1}s")
    } else {
        format!("{:.0}s", secs.round())
    };
    let status = if failed {
        "\x1b[31m✗ failed\x1b[0m"
    } else {
        "\x1b[32m✓ ready\x1b[0m"
    };
    let mut parts = vec![status.to_string(), format!("\x1b[2m{time}\x1b[0m")];
    if input_tokens > 0 || output_tokens > 0 {
        parts.push(format!(
            "\x1b[2m{}↓ {}↑\x1b[0m",
            format_token_count(input_tokens),
            format_token_count(output_tokens)
        ));
    }
    format!("  {}", parts.join(" · "))
}

#[must_use]
pub fn format_token_count(n: u32) -> String {
    if n >= 10_000 {
        format!("{:.1}k", f64::from(n) / 1000.0)
    } else if n >= 1000 {
        format!("{:.1}k", f64::from(n) / 1000.0)
    } else {
        n.to_string()
    }
}

/// Compact tool timeline line: `▸ read  path/to/file`
#[must_use]
pub fn format_tool_timeline_line(label: &str, detail: &str) -> String {
    let detail = detail.trim();
    if detail.is_empty() {
        format!("  \x1b[2m▸\x1b[0m \x1b[1m{label}\x1b[0m")
    } else {
        format!("  \x1b[2m▸\x1b[0m \x1b[1m{label}\x1b[0m  \x1b[2m{detail}\x1b[0m")
    }
}

/// User turn header for calm chat chrome.
#[must_use]
pub fn format_user_turn_header() -> String {
    "  \x1b[2myou\x1b[0m".to_string()
}

/// Dim separator between turns.
#[must_use]
pub fn format_turn_separator_line() -> String {
    format!("  \x1b[2m{}\x1b[0m", "·".repeat(28))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalTheme {
    Emoji = 0,
    Chrome = 1,
    Classic = 2,
}

impl TerminalTheme {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "clawie1" | "emoji" | "default" => Some(Self::Emoji),
            "chrome" | "black-white" | "black-and-white" | "bw" => Some(Self::Chrome),
            "classic" | "red" => Some(Self::Classic),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Emoji => "clawie1",
            Self::Chrome => "chrome",
            Self::Classic => "classic",
        }
    }

    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            Self::Emoji => "red accent with emoji status markers",
            Self::Chrome => "black and white with emoji status markers",
            Self::Classic => "red accent without emoji status markers",
        }
    }

    #[must_use]
    pub fn emojis_enabled(self) -> bool {
        matches!(self, Self::Emoji | Self::Chrome)
    }

    #[must_use]
    pub fn banner_color(self) -> Color {
        match self {
            Self::Emoji | Self::Classic => Color::Red,
            Self::Chrome => Color::Grey,
        }
    }

    fn from_storage(value: u8) -> Self {
        match value {
            1 => Self::Chrome,
            2 => Self::Classic,
            _ => Self::Emoji,
        }
    }
}

#[must_use]
pub fn active_terminal_theme() -> TerminalTheme {
    TerminalTheme::from_storage(ACTIVE_TERMINAL_THEME.load(Ordering::Relaxed))
}

pub fn set_active_terminal_theme(theme: TerminalTheme) {
    ACTIVE_TERMINAL_THEME.store(theme as u8, Ordering::Relaxed);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorTheme {
    heading: Color,
    emphasis: Color,
    strong: Color,
    inline_code: Color,
    link: Color,
    quote: Color,
    table_border: Color,
    code_block_border: Color,
    spinner_active: Color,
    spinner_done: Color,
    spinner_failed: Color,
}

impl Default for ColorTheme {
    fn default() -> Self {
        Self::for_terminal_theme(TerminalTheme::Emoji)
    }
}

impl ColorTheme {
    #[must_use]
    pub fn for_terminal_theme(theme: TerminalTheme) -> Self {
        match theme {
            TerminalTheme::Emoji | TerminalTheme::Classic => Self {
                heading: Color::Red,
                emphasis: Color::Magenta,
                strong: Color::Yellow,
                inline_code: Color::Green,
                link: Color::Blue,
                quote: Color::DarkGrey,
                table_border: Color::DarkRed,
                code_block_border: Color::DarkGrey,
                // Soft red pulse on the active frame reads smoother than pure white.
                spinner_active: Color::DarkRed,
                spinner_done: Color::Green,
                spinner_failed: Color::Red,
            },
            TerminalTheme::Chrome => Self {
                heading: Color::White,
                emphasis: Color::Grey,
                strong: Color::White,
                inline_code: Color::Grey,
                link: Color::White,
                quote: Color::DarkGrey,
                table_border: Color::Grey,
                code_block_border: Color::DarkGrey,
                spinner_active: Color::Grey,
                spinner_done: Color::White,
                spinner_failed: Color::Grey,
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct Spinner {
    frame_index: usize,
    started_at: Instant,
}

impl Default for Spinner {
    fn default() -> Self {
        Self {
            frame_index: 0,
            started_at: Instant::now(),
        }
    }
}

impl Spinner {
    /// Braille spinner frames — reads as continuous motion in the terminal.
    const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    const TRAILING_DOTS: [&str; 4] = [".  ", ".. ", "...", "   "];

    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// True once the spinner has been visible long enough to paint (or was forced).
    #[must_use]
    pub fn should_paint(&self) -> bool {
        self.started_at.elapsed() >= SPINNER_DELAY
    }

    pub fn tick(
        &mut self,
        label: &str,
        theme: &ColorTheme,
        out: &mut impl Write,
    ) -> io::Result<()> {
        // Reply text owns the terminal — do not fight it for the current line.
        if SPINNER_HELD.load(Ordering::Relaxed) {
            return Ok(());
        }
        // Avoid a flash for sub-300ms turns.
        if !self.should_paint() {
            return Ok(());
        }
        let frame = Self::FRAMES[self.frame_index % Self::FRAMES.len()];
        let animated_label = if let Some(prefix) = label.strip_suffix("...") {
            let dots = Self::TRAILING_DOTS[self.frame_index % Self::TRAILING_DOTS.len()];
            format!("{prefix}{dots}")
        } else {
            label.to_string()
        };
        let elapsed_seconds = self.started_at.elapsed().as_secs();
        let display_label = if elapsed_seconds > 0 {
            format!("{animated_label}  \x1b[2m{elapsed_seconds}s\x1b[0m")
        } else {
            animated_label
        };
        self.frame_index += 1;
        let _guard = lock_stdout();
        // Always redraw from column 0. End with \r so the cursor sits at the
        // start of this line for the next tick / reply takeover.
        queue!(
            out,
            MoveToColumn(0),
            Clear(ClearType::CurrentLine),
            SetForegroundColor(theme.spinner_active),
            Print(format!("  {frame} {display_label}")),
            ResetColor,
            Print("\r")
        )?;
        out.flush()
    }

    pub fn finish(
        &mut self,
        label: &str,
        theme: &ColorTheme,
        out: &mut impl Write,
    ) -> io::Result<()> {
        self.frame_index = 0;
        // If we never painted, stay silent (fast turn).
        if !self.should_paint() && !SPINNER_HELD.load(Ordering::Relaxed) {
            return Ok(());
        }
        prepare_cooked_stdout();
        let _guard = lock_stdout();
        // When reply already printed, skip the extra "ready" line — footer handles it.
        if SPINNER_HELD.load(Ordering::Relaxed) {
            execute!(
                out,
                MoveToColumn(0),
                Clear(ClearType::CurrentLine)
            )?;
            return out.flush();
        }
        execute!(
            out,
            MoveToColumn(0),
            Clear(ClearType::CurrentLine),
            SetForegroundColor(theme.spinner_done),
            Print(format!("  ✓ {label}\r\n")),
            ResetColor
        )?;
        out.flush()
    }

    /// Clear spinner line without a status message (footer will follow).
    pub fn clear_line(&mut self, out: &mut impl Write) -> io::Result<()> {
        self.frame_index = 0;
        prepare_cooked_stdout();
        let _guard = lock_stdout();
        execute!(out, MoveToColumn(0), Clear(ClearType::CurrentLine))?;
        out.flush()
    }

    pub fn fail(
        &mut self,
        label: &str,
        theme: &ColorTheme,
        out: &mut impl Write,
    ) -> io::Result<()> {
        self.frame_index = 0;
        prepare_cooked_stdout();
        let _guard = lock_stdout();
        execute!(
            out,
            MoveToColumn(0),
            Clear(ClearType::CurrentLine),
            SetForegroundColor(theme.spinner_failed),
            Print(format!("  ✗ {label}\r\n")),
            ResetColor
        )?;
        out.flush()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ListKind {
    Unordered,
    Ordered { next_index: u64 },
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct TableState {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    current_row: Vec<String>,
    current_cell: String,
    in_head: bool,
}

impl TableState {
    fn push_cell(&mut self) {
        let cell = self.current_cell.trim().to_string();
        self.current_row.push(cell);
        self.current_cell.clear();
    }

    fn finish_row(&mut self) {
        if self.current_row.is_empty() {
            return;
        }
        let row = std::mem::take(&mut self.current_row);
        if self.in_head {
            self.headers = row;
        } else {
            self.rows.push(row);
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct RenderState {
    emphasis: usize,
    strong: usize,
    heading_level: Option<u8>,
    quote: usize,
    list_stack: Vec<ListKind>,
    link_stack: Vec<LinkState>,
    table: Option<TableState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LinkState {
    destination: String,
    text: String,
}

impl RenderState {
    fn style_text(&self, text: &str, theme: &ColorTheme) -> String {
        let mut style = text.stylize();

        let is_phase = text.trim_start().starts_with("Phase ");
        if is_phase {
            style = style.bold().cyan();
        }

        if matches!(self.heading_level, Some(1 | 2)) || self.strong > 0 {
            style = style.bold();
        }
        if self.emphasis > 0 {
            style = style.italic();
        }

        if is_phase {
            // keep bold cyan
        } else if let Some(level) = self.heading_level {
            style = match level {
                1 => style.with(theme.heading),
                2 => style.white(),
                3 => style.with(Color::Blue),
                _ => style.with(Color::Grey),
            };
        } else if self.strong > 0 {
            style = style.with(theme.strong);
        } else if self.emphasis > 0 {
            style = style.with(theme.emphasis);
        }

        if self.quote > 0 {
            style = style.with(theme.quote);
        }

        format!("{style}")
    }

    fn append_raw(&mut self, output: &mut String, text: &str) {
        if let Some(link) = self.link_stack.last_mut() {
            link.text.push_str(text);
        } else if let Some(table) = self.table.as_mut() {
            table.current_cell.push_str(text);
        } else {
            output.push_str(text);
        }
    }

    fn append_styled(&mut self, output: &mut String, text: &str, theme: &ColorTheme) {
        let styled = self.style_text(text, theme);
        self.append_raw(output, &styled);
    }
}

#[derive(Debug)]
pub struct TerminalRenderer {
    syntax_set: SyntaxSet,
    syntax_theme: Theme,
    terminal_theme: TerminalTheme,
    color_theme: ColorTheme,
}

/// Keep replies intentionally narrow so long answers stack vertically
/// (one idea / step / command under another) instead of wide walls of text.
const VERTICAL_REPLY_WRAP_WIDTH_MIN: usize = 40;
const VERTICAL_REPLY_WRAP_WIDTH_MAX: usize = 56;
const VERTICAL_REPLY_WRAP_WIDTH_FALLBACK: usize = 48;

fn reply_wrap_width() -> usize {
    crossterm::terminal::size()
        .ok()
        .map(|(cols, _)| {
            // Prefer ~half a typical terminal so blocks stack top-to-bottom.
            let usable = (cols as usize).saturating_sub(8).min(cols as usize * 2 / 3);
            usable.clamp(VERTICAL_REPLY_WRAP_WIDTH_MIN, VERTICAL_REPLY_WRAP_WIDTH_MAX)
        })
        .unwrap_or(VERTICAL_REPLY_WRAP_WIDTH_FALLBACK)
}

impl Default for TerminalRenderer {
    fn default() -> Self {
        let syntax_set = SyntaxSet::load_defaults_newlines();
        let syntax_theme = ThemeSet::load_defaults()
            .themes
            .remove("base16-ocean.dark")
            .unwrap_or_default();
        let terminal_theme = active_terminal_theme();
        Self {
            syntax_set,
            syntax_theme,
            terminal_theme,
            color_theme: ColorTheme::for_terminal_theme(terminal_theme),
        }
    }
}

impl TerminalRenderer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn color_theme(&self) -> &ColorTheme {
        &self.color_theme
    }

    #[must_use]
    pub fn terminal_theme(&self) -> TerminalTheme {
        self.terminal_theme
    }

    #[must_use]
    pub fn render_markdown(&self, markdown: &str) -> String {
        let mut output = String::new();
        let mut state = RenderState::default();
        let mut code_language = String::new();
        let mut code_buffer = String::new();
        let mut in_code_block = false;

        for event in Parser::new_ext(markdown, Options::all()) {
            self.render_event(
                event,
                &mut state,
                &mut output,
                &mut code_buffer,
                &mut code_language,
                &mut in_code_block,
            );
        }

        output.trim_end().to_string()
    }

    #[must_use]
    pub fn markdown_to_ansi(&self, markdown: &str) -> String {
        self.render_markdown(markdown)
    }

    #[must_use]
    pub fn vertical_markdown_to_ansi(&self, markdown: &str) -> String {
        // Custom vertical stack — do NOT route structured bullets through the
        // general markdown list renderer (that path was producing staircase indents).
        self.render_vertical_chat(markdown)
    }

    /// Soft chat-style header for assistant replies.
    #[must_use]
    pub fn reply_header_line(&self) -> String {
        let accent = match self.terminal_theme {
            TerminalTheme::Chrome => "\x1b[37m",
            _ => "\x1b[31m",
        };
        if self.terminal_theme.emojis_enabled() {
            format!("{accent}✦\x1b[0m \x1b[1mclawie\x1b[0m")
        } else {
            format!("{accent}·\x1b[0m \x1b[1mclawie\x1b[0m")
        }
    }

    /// Dim horizontal rule used as a quiet turn separator.
    #[must_use]
    pub fn turn_separator(&self) -> String {
        let width = reply_wrap_width().min(36);
        format!("\x1b[2m{}\x1b[0m", "·".repeat(width.max(10)))
    }

    /// Build a left-aligned vertical chat body:
    ///   • sentence one
    ///   • sentence two
    ///   ╭─ bash
    ///   │ command
    ///   ╰─
    fn render_vertical_chat(&self, markdown: &str) -> String {
        let structured = structure_reply_markdown(markdown);
        let wrap = reply_wrap_width();
        let mut lines: Vec<String> = Vec::new();
        let mut in_fence = false;
        let mut fence_lang = String::new();
        let mut fence_body = String::new();

        let push_blank = |lines: &mut Vec<String>| {
            if lines.last().is_some_and(|l| !l.is_empty()) {
                lines.push(String::new());
            }
        };

        for raw in structured.lines() {
            // Never inherit accidental leading spaces — force column 0 content.
            let line = raw.trim_start();

            if line.starts_with("```") || line.starts_with("~~~") {
                if !in_fence {
                    in_fence = true;
                    fence_lang = line
                        .trim_start_matches('`')
                        .trim_start_matches('~')
                        .trim()
                        .to_string();
                    if fence_lang.is_empty() {
                        fence_lang = "code".to_string();
                    }
                    fence_body.clear();
                } else {
                    in_fence = false;
                    push_blank(&mut lines);
                    lines.extend(self.format_code_rail_block(&fence_lang, &fence_body));
                    push_blank(&mut lines);
                    fence_lang.clear();
                    fence_body.clear();
                }
                continue;
            }

            if in_fence {
                if !fence_body.is_empty() {
                    fence_body.push('\n');
                }
                fence_body.push_str(raw.trim_end());
                continue;
            }

            if line.is_empty() {
                push_blank(&mut lines);
                continue;
            }

            if line.starts_with('#') {
                push_blank(&mut lines);
                let heading = line.trim_start_matches('#').trim();
                lines.push(format!(
                    "{}",
                    heading.bold().with(self.color_theme.heading)
                ));
                push_blank(&mut lines);
                continue;
            }

            if line.starts_with('>') {
                let quote = line.trim_start_matches('>').trim();
                for wrapped in wrap_with_prefix(quote, "│ ", "│ ", wrap) {
                    lines.push(format!("{}", wrapped.with(self.color_theme.quote)));
                }
                continue;
            }

            if let Some((marker, rest)) = split_list_marker(line) {
                let is_ordered = marker.as_bytes().first().is_some_and(u8::is_ascii_digit);
                let (first_prefix, cont_prefix) = if is_ordered {
                    (marker.to_string(), " ".repeat(marker.chars().count()))
                } else {
                    // Always top-level bullets — never nested staircase.
                    ("• ".to_string(), "  ".to_string())
                };
                for wrapped in wrap_with_prefix(rest, &first_prefix, &cont_prefix, wrap) {
                    lines.push(wrapped);
                }
                continue;
            }

            // Plain prose line → top-level bullet, always left-aligned.
            for wrapped in wrap_with_prefix(line, "• ", "  ", wrap) {
                lines.push(wrapped);
            }
        }

        if in_fence {
            push_blank(&mut lines);
            lines.extend(self.format_code_rail_block(&fence_lang, &fence_body));
        }

        // Collapse trailing blanks.
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines.join("\n")
    }

    fn format_code_rail_block(&self, language: &str, code: &str) -> Vec<String> {
        let border = self.color_theme.code_block_border;
        let label = if language.is_empty() { "code" } else { language };
        let mut lines = Vec::new();
        lines.push(format!("{}", format!("╭─ {label}").bold().with(border)));
        let highlighted = self.highlight_code(code, language);
        for line in highlighted.split_inclusive('\n') {
            let content = line.strip_suffix('\n').unwrap_or(line);
            lines.push(format!("{}{content}", format!("│ ").with(border)));
        }
        if code.is_empty() {
            lines.push(format!("{}", "│ ".with(border)));
        }
        lines.push(format!("{}", "╰─".bold().with(border)));
        lines
    }

    #[allow(clippy::too_many_lines)]
    fn render_event(
        &self,
        event: Event<'_>,
        state: &mut RenderState,
        output: &mut String,
        code_buffer: &mut String,
        code_language: &mut String,
        in_code_block: &mut bool,
    ) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                Self::start_heading(state, level as u8, output);
            }
            Event::End(TagEnd::Paragraph) => output.push_str("\n\n"),
            Event::Start(Tag::BlockQuote(..)) => self.start_quote(state, output),
            Event::End(TagEnd::BlockQuote(..)) => {
                state.quote = state.quote.saturating_sub(1);
                output.push('\n');
            }
            Event::End(TagEnd::Heading(..)) => {
                state.heading_level = None;
                output.push_str("\n\n");
            }
            Event::End(TagEnd::Item) | Event::SoftBreak | Event::HardBreak => {
                state.append_raw(output, "\n");
            }
            Event::Start(Tag::List(first_item)) => {
                let kind = match first_item {
                    Some(index) => ListKind::Ordered { next_index: index },
                    None => ListKind::Unordered,
                };
                state.list_stack.push(kind);
            }
            Event::End(TagEnd::List(..)) => {
                state.list_stack.pop();
                output.push('\n');
            }
            Event::Start(Tag::Item) => Self::start_item(state, output),
            Event::Start(Tag::CodeBlock(kind)) => {
                *in_code_block = true;
                *code_language = match kind {
                    CodeBlockKind::Indented => String::from("text"),
                    CodeBlockKind::Fenced(lang) => lang.to_string(),
                };
                code_buffer.clear();
                if *code_language != "diff" {
                    self.start_code_block(code_language, output);
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                self.finish_code_block(code_buffer, code_language, output);
                *in_code_block = false;
                code_language.clear();
                code_buffer.clear();
            }
            Event::Start(Tag::Emphasis) => state.emphasis += 1,
            Event::End(TagEnd::Emphasis) => state.emphasis = state.emphasis.saturating_sub(1),
            Event::Start(Tag::Strong) => state.strong += 1,
            Event::End(TagEnd::Strong) => state.strong = state.strong.saturating_sub(1),
            Event::Code(code) => {
                let rendered =
                    format!("{}", format!("`{code}`").with(self.color_theme.inline_code));
                state.append_raw(output, &rendered);
            }
            Event::Rule => output.push_str("---\n"),
            Event::Text(text) => {
                self.push_text(text.as_ref(), state, output, code_buffer, *in_code_block);
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                state.append_raw(output, &html);
            }
            Event::FootnoteReference(reference) => {
                state.append_raw(output, &format!("[{reference}]"));
            }
            Event::TaskListMarker(done) => {
                let marker = if done {
                    "\x1b[32m✔\x1b[0m "
                } else {
                    "\x1b[33m●\x1b[0m "
                };
                if output.ends_with("• ") {
                    output.truncate(output.len() - "• ".len());
                }
                state.append_raw(output, marker);
            }
            Event::InlineMath(math) | Event::DisplayMath(math) => {
                state.append_raw(output, &math);
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                state.link_stack.push(LinkState {
                    destination: dest_url.to_string(),
                    text: String::new(),
                });
            }
            Event::End(TagEnd::Link) => {
                if let Some(link) = state.link_stack.pop() {
                    let label = if link.text.is_empty() {
                        link.destination.clone()
                    } else {
                        link.text
                    };
                    let rendered = format!(
                        "{}",
                        format!("[{label}]({})", link.destination)
                            .underlined()
                            .with(self.color_theme.link)
                    );
                    state.append_raw(output, &rendered);
                }
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                let rendered = format!(
                    "{}",
                    format!("[image:{dest_url}]").with(self.color_theme.link)
                );
                state.append_raw(output, &rendered);
            }
            Event::Start(Tag::Table(..)) => state.table = Some(TableState::default()),
            Event::End(TagEnd::Table) => {
                if let Some(table) = state.table.take() {
                    output.push_str(&self.render_table(&table));
                    output.push_str("\n\n");
                }
            }
            Event::Start(Tag::TableHead) => {
                if let Some(table) = state.table.as_mut() {
                    table.in_head = true;
                }
            }
            Event::End(TagEnd::TableHead) => {
                if let Some(table) = state.table.as_mut() {
                    table.finish_row();
                    table.in_head = false;
                }
            }
            Event::Start(Tag::TableRow) => {
                if let Some(table) = state.table.as_mut() {
                    table.current_row.clear();
                    table.current_cell.clear();
                }
            }
            Event::End(TagEnd::TableRow) => {
                if let Some(table) = state.table.as_mut() {
                    table.finish_row();
                }
            }
            Event::Start(Tag::TableCell) => {
                if let Some(table) = state.table.as_mut() {
                    table.current_cell.clear();
                }
            }
            Event::End(TagEnd::TableCell) => {
                if let Some(table) = state.table.as_mut() {
                    table.push_cell();
                }
            }
            Event::Start(Tag::Paragraph | Tag::MetadataBlock(..) | _)
            | Event::End(TagEnd::Image | TagEnd::MetadataBlock(..) | _) => {}
        }
    }

    fn start_heading(state: &mut RenderState, level: u8, output: &mut String) {
        state.heading_level = Some(level);
        if !output.is_empty() {
            output.push('\n');
        }
    }

    fn start_quote(&self, state: &mut RenderState, output: &mut String) {
        state.quote += 1;
        let _ = write!(output, "{}", "│ ".with(self.color_theme.quote));
    }

    fn start_item(state: &mut RenderState, output: &mut String) {
        let depth = state.list_stack.len().saturating_sub(1);
        output.push_str(&"  ".repeat(depth));

        let marker = match state.list_stack.last_mut() {
            Some(ListKind::Ordered { next_index }) => {
                let value = *next_index;
                *next_index += 1;
                format!("{value}. ")
            }
            _ => "• ".to_string(),
        };
        output.push_str(&marker);
    }

    fn start_code_block(&self, code_language: &str, output: &mut String) {
        let label = if code_language.is_empty() {
            "code".to_string()
        } else {
            code_language.to_string()
        };
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        let _ = writeln!(
            output,
            "{}",
            format!("╭─ {label}")
                .bold()
                .with(self.color_theme.code_block_border)
        );
    }

    fn finish_code_block(&self, code_buffer: &str, code_language: &str, output: &mut String) {
        if code_language == "diff" {
            output.push_str(&self.render_pretty_diff(code_buffer));
        } else {
            let rail = format!("{}", "│ ".with(self.color_theme.code_block_border));
            let highlighted = self.highlight_code(code_buffer, code_language);
            for line in highlighted.split_inclusive('\n') {
                let (content, newline) = line
                    .strip_suffix('\n')
                    .map_or((line, false), |body| (body, true));
                // Skip pure-empty trailing lines from the highlighter.
                if content.is_empty() && !newline {
                    continue;
                }
                output.push_str(&rail);
                output.push_str(content);
                if newline {
                    output.push('\n');
                }
            }
            if !output.ends_with('\n') {
                output.push('\n');
            }
            let _ = writeln!(
                output,
                "{}",
                "╰─".bold().with(self.color_theme.code_block_border)
            );
            output.push('\n');
        }
    }

    fn render_pretty_diff(&self, diff: &str) -> String {
        let mut rendered = String::new();
        let border_color = self.color_theme.code_block_border;
        let _ = writeln!(
            &mut rendered,
            "{}",
            "┌── Code Modifications ──────────────────────────────────────"
                .bold()
                .with(border_color)
        );

        for line in diff.lines() {
            if line.starts_with('+') {
                let _ = writeln!(
                    &mut rendered,
                    "{}\x1b[38;5;70m{}\x1b[0m",
                    "│ ".bold().with(border_color),
                    line.trim_end()
                );
            } else if line.starts_with('-') {
                let _ = writeln!(
                    &mut rendered,
                    "{}\x1b[38;5;203m{}\x1b[0m",
                    "│ ".bold().with(border_color),
                    line.trim_end()
                );
            } else if line.starts_with("@@") {
                let _ = writeln!(
                    &mut rendered,
                    "{}\x1b[36m{}\x1b[0m",
                    "│ ".bold().with(border_color),
                    line.trim_end()
                );
            } else {
                let _ = writeln!(
                    &mut rendered,
                    "{}{}",
                    "│ ".bold().with(border_color),
                    line.trim_end()
                );
            }
        }

        let _ = writeln!(
            &mut rendered,
            "{}",
            "└────────────────────────────────────────────────────────────"
                .bold()
                .with(border_color)
        );
        rendered
    }

    fn push_text(
        &self,
        text: &str,
        state: &mut RenderState,
        output: &mut String,
        code_buffer: &mut String,
        in_code_block: bool,
    ) {
        if in_code_block {
            code_buffer.push_str(text);
        } else {
            state.append_styled(output, text, &self.color_theme);
        }
    }

    fn render_table(&self, table: &TableState) -> String {
        let mut rows = Vec::new();
        if !table.headers.is_empty() {
            rows.push(table.headers.clone());
        }
        rows.extend(table.rows.iter().cloned());

        if rows.is_empty() {
            return String::new();
        }

        let column_count = rows.iter().map(Vec::len).max().unwrap_or(0);
        let widths = (0..column_count)
            .map(|column| {
                rows.iter()
                    .filter_map(|row| row.get(column))
                    .map(|cell| visible_width(cell))
                    .max()
                    .unwrap_or(0)
            })
            .collect::<Vec<_>>();

        let border = format!("{}", "│".with(self.color_theme.table_border));
        let separator = widths
            .iter()
            .map(|width| "─".repeat(*width + 2))
            .collect::<Vec<_>>()
            .join(&format!("{}", "┼".with(self.color_theme.table_border)));
        let separator = format!("{border}{separator}{border}");

        let mut output = String::new();
        if !table.headers.is_empty() {
            output.push_str(&self.render_table_row(&table.headers, &widths, true));
            output.push('\n');
            output.push_str(&separator);
            if !table.rows.is_empty() {
                output.push('\n');
            }
        }

        for (index, row) in table.rows.iter().enumerate() {
            output.push_str(&self.render_table_row(row, &widths, false));
            if index + 1 < table.rows.len() {
                output.push('\n');
            }
        }

        output
    }

    fn render_table_row(&self, row: &[String], widths: &[usize], is_header: bool) -> String {
        let border = format!("{}", "│".with(self.color_theme.table_border));
        let mut line = String::new();
        line.push_str(&border);

        for (index, width) in widths.iter().enumerate() {
            let cell = row.get(index).map_or("", String::as_str);
            line.push(' ');
            if is_header {
                let _ = write!(line, "{}", cell.bold().with(self.color_theme.heading));
            } else {
                line.push_str(cell);
            }
            let padding = width.saturating_sub(visible_width(cell));
            line.push_str(&" ".repeat(padding + 1));
            line.push_str(&border);
        }

        line
    }

    #[must_use]
    pub fn highlight_code(&self, code: &str, language: &str) -> String {
        let syntax = self
            .syntax_set
            .find_syntax_by_token(language)
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());
        let mut syntax_highlighter = HighlightLines::new(syntax, &self.syntax_theme);
        let mut colored_output = String::new();

        for line in LinesWithEndings::from(code) {
            match syntax_highlighter.highlight_line(line, &self.syntax_set) {
                Ok(ranges) => {
                    let escaped = as_24_bit_terminal_escaped(&ranges[..], false);
                    colored_output.push_str(&apply_code_block_background(&escaped));
                }
                Err(_) => colored_output.push_str(&apply_code_block_background(line)),
            }
        }

        colored_output
    }

    pub fn stream_markdown(&self, markdown: &str, out: &mut impl Write) -> io::Result<()> {
        let rendered_markdown = self.markdown_to_ansi(markdown);
        write!(out, "{rendered_markdown}")?;
        if !rendered_markdown.ends_with('\n') {
            writeln!(out)?;
        }
        out.flush()
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MarkdownStreamState {
    pending: String,
}

impl MarkdownStreamState {
    #[must_use]
    pub fn push(&mut self, renderer: &TerminalRenderer, delta: &str) -> Option<String> {
        self.pending.push_str(delta);
        let split = find_stream_safe_boundary(&self.pending)?;
        let ready = self.pending[..split].to_string();
        self.pending.drain(..split);
        Some(renderer.vertical_markdown_to_ansi(&ready))
    }

    #[must_use]
    pub fn flush(&mut self, renderer: &TerminalRenderer) -> Option<String> {
        if self.pending.trim().is_empty() {
            self.pending.clear();
            None
        } else {
            let pending = std::mem::take(&mut self.pending);
            Some(renderer.vertical_markdown_to_ansi(&pending))
        }
    }
}

fn reflow_markdown_for_vertical_layout(markdown: &str, wrap_width: usize) -> String {
    let mut output = Vec::new();
    let mut in_fence = false;

    for line in markdown.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            output.push(line.to_string());
            continue;
        }

        if in_fence
            || trimmed.is_empty()
            || trimmed.starts_with('|')
            || trimmed.starts_with("---")
            || trimmed.starts_with("===")
        {
            output.push(line.to_string());
            continue;
        }

        if let Some((first_prefix, continuation_prefix, content)) = split_list_item(line) {
            output.extend(wrap_with_prefix(
                &content,
                &first_prefix,
                &continuation_prefix,
                wrap_width,
            ));
        } else {
            output.extend(wrap_with_prefix(trimmed, "", "", wrap_width));
        }
    }

    output.join("\n")
}

/// Rewrite assistant markdown so long answers read as a vertical stack:
/// one sentence / step / block under another, with blank lines between sections.
fn structure_reply_markdown(markdown: &str) -> String {
    let mut output: Vec<String> = Vec::new();
    let mut in_fence = false;
    let mut fence_buf: Vec<String> = Vec::new();
    let mut para_buf: Vec<String> = Vec::new();

    let flush_para = |para_buf: &mut Vec<String>, output: &mut Vec<String>| {
        if para_buf.is_empty() {
            return;
        }
        let text = para_buf
            .drain(..)
            .map(|line| line.trim().to_string())
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if text.is_empty() {
            return;
        }
        // Keep short one-liners as a single bullet; split denser prose.
        let sentences = split_into_sentences(&text)
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        if sentences.is_empty() {
            output.push(format!("- {text}"));
        } else {
            for sentence in sentences {
                output.push(format!("- {sentence}"));
            }
        }
        output.push(String::new());
    };

    for line in markdown.lines() {
        let trimmed = line.trim_start();

        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            flush_para(&mut para_buf, &mut output);
            if !in_fence {
                in_fence = true;
                fence_buf.clear();
                fence_buf.push(trimmed.to_string());
            } else {
                in_fence = false;
                fence_buf.push(trimmed.to_string());
                // Blank line before / after code so it sits alone in the stack.
                if output.last().is_some_and(|l| !l.is_empty()) {
                    output.push(String::new());
                }
                output.extend(fence_buf.drain(..));
                output.push(String::new());
            }
            continue;
        }

        if in_fence {
            // Preserve code body (trim only trailing whitespace noise).
            fence_buf.push(line.trim_end().to_string());
            continue;
        }

        if trimmed.is_empty() {
            flush_para(&mut para_buf, &mut output);
            continue;
        }

        if trimmed.starts_with('#') {
            flush_para(&mut para_buf, &mut output);
            if output.last().is_some_and(|l| !l.is_empty()) {
                output.push(String::new());
            }
            output.push(trimmed.to_string());
            output.push(String::new());
            continue;
        }

        if is_markdown_list_item(trimmed) || trimmed.starts_with('>') || trimmed.starts_with('|') {
            flush_para(&mut para_buf, &mut output);
            // Long list items → one sentence per line under the same marker when dense.
            if let Some((marker, rest)) = split_list_marker(trimmed) {
                let sentences = split_into_sentences(rest)
                    .into_iter()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>();
                // Always same-level items — nested "  - " caused staircase indents.
                if sentences.is_empty() {
                    output.push(format!("{marker}{rest}"));
                } else {
                    for sentence in sentences {
                        output.push(format!("{marker}{sentence}"));
                    }
                }
            } else {
                output.push(trimmed.to_string());
            }
            continue;
        }

        para_buf.push(trimmed.to_string());
    }

    flush_para(&mut para_buf, &mut output);
    if in_fence {
        // Unclosed fence — still emit what we have.
        if output.last().is_some_and(|l| !l.is_empty()) {
            output.push(String::new());
        }
        output.extend(fence_buf.drain(..));
    }

    // Collapse runs of more than one blank line.
    let mut cleaned: Vec<String> = Vec::with_capacity(output.len());
    for line in output {
        if line.is_empty() && cleaned.last().is_some_and(String::is_empty) {
            continue;
        }
        cleaned.push(line);
    }
    while cleaned.last().is_some_and(String::is_empty) {
        cleaned.pop();
    }
    cleaned.join("\n")
}

fn is_markdown_list_item(trimmed: &str) -> bool {
    if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("+ ") {
        return true;
    }
    let digits = trimmed
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .count();
    digits > 0 && trimmed[digits..].starts_with(". ")
}

fn split_list_marker(trimmed: &str) -> Option<(&str, &str)> {
    for prefix in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return Some((prefix, rest.trim()));
        }
    }
    let digits = trimmed
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .count();
    if digits > 0 && trimmed[digits..].starts_with(". ") {
        let marker = &trimmed[..digits + 2];
        let rest = trimmed[digits + 2..].trim();
        return Some((marker, rest));
    }
    None
}

fn split_into_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        current.push(ch);
        if matches!(ch, '.' | '!' | '?') {
            let next_non_space = chars.clone().find(|c| !c.is_whitespace());
            let should_split = next_non_space
                .map(|next| {
                    next.is_uppercase()
                        || next.is_ascii_digit()
                        || matches!(next, '"' | '\'' | '`' | '(' | '[')
                })
                .unwrap_or(true);
            if should_split {
                let sentence = current.trim();
                if !sentence.is_empty() {
                    sentences.push(sentence.to_string());
                }
                current.clear();
            }
        }
    }

    let tail = current.trim();
    if !tail.is_empty() {
        sentences.push(tail.to_string());
    }

    sentences
}

fn split_list_item(line: &str) -> Option<(String, String, String)> {
    let indent_len = line.len().saturating_sub(line.trim_start().len());
    let trimmed = &line[indent_len..];

    let marker_len = if matches!(
        trimmed.as_bytes().first().copied(),
        Some(b'-' | b'*' | b'+')
    ) && trimmed.as_bytes().get(1) == Some(&b' ')
    {
        2
    } else {
        let mut digits = 0usize;
        for ch in trimmed.chars() {
            if ch.is_ascii_digit() {
                digits += ch.len_utf8();
                continue;
            }
            break;
        }
        if digits > 0 && trimmed[digits..].strip_prefix(". ").is_some() {
            digits + 2
        } else {
            return None;
        }
    };

    let prefix = trimmed[..marker_len].to_string();
    let content = trimmed[marker_len..].trim().to_string();
    let first_prefix = format!("{}{}", " ".repeat(indent_len), prefix);
    let continuation_prefix = " ".repeat(first_prefix.chars().count());
    Some((first_prefix, continuation_prefix, content))
}

fn wrap_with_prefix(
    text: &str,
    first_prefix: &str,
    continuation_prefix: &str,
    wrap_width: usize,
) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut prefix = first_prefix;
    let mut available = wrap_width.saturating_sub(prefix.chars().count());

    for word in text.split_whitespace() {
        let next_len = if current.is_empty() {
            word.chars().count()
        } else {
            current.chars().count() + 1 + word.chars().count()
        };

        if !current.is_empty() && next_len > available {
            lines.push(format!("{prefix}{current}"));
            current.clear();
            current.push_str(word);
            prefix = continuation_prefix;
            available = wrap_width.saturating_sub(prefix.chars().count());
        } else if current.is_empty() {
            current.push_str(word);
        } else {
            current.push(' ');
            current.push_str(word);
        }
    }

    if current.is_empty() {
        if lines.is_empty() {
            lines.push(prefix.to_string());
        }
    } else {
        lines.push(format!("{prefix}{current}"));
    }

    lines
}

fn apply_code_block_background(line: &str) -> String {
    let trimmed = line.trim_end_matches('\n');
    let trailing_newline = if trimmed.len() == line.len() {
        ""
    } else {
        "\n"
    };
    let with_background = trimmed.replace("\u{1b}[0m", "\u{1b}[0;48;5;236m");
    format!("\u{1b}[48;5;236m{with_background}\u{1b}[0m{trailing_newline}")
}

fn find_stream_safe_boundary(markdown: &str) -> Option<usize> {
    let mut in_fence = false;
    let mut last_boundary = None;

    for (offset, line) in markdown.split_inclusive('\n').scan(0usize, |cursor, line| {
        let start = *cursor;
        *cursor += line.len();
        Some((start, line))
    }) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            if !in_fence {
                last_boundary = Some(offset + line.len());
            }
            continue;
        }

        if in_fence {
            continue;
        }

        if trimmed.is_empty() {
            last_boundary = Some(offset + line.len());
        }
    }

    last_boundary
}

fn visible_width(input: &str) -> usize {
    strip_ansi(input).chars().count()
}

fn strip_ansi(input: &str) -> String {
    let mut output = String::new();
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            output.push(ch);
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::{
        format_tool_timeline_line, format_turn_footer, release_spinner, strip_ansi,
        MarkdownStreamState, Spinner, TerminalRenderer, TerminalTheme, SPINNER_DELAY,
    };
    use crossterm::style::Color;
    use std::time::{Duration, Instant};

    #[test]
    fn renders_markdown_with_styling_and_lists() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output = terminal_renderer
            .render_markdown("# Heading\n\nThis is **bold** and *italic*.\n\n- item\n\n`code`");

        assert!(markdown_output.contains("Heading"));
        assert!(markdown_output.contains("• item"));
        assert!(markdown_output.contains("code"));
        assert!(markdown_output.contains('\u{1b}'));
    }

    #[test]
    fn renders_links_as_colored_markdown_labels() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output =
            terminal_renderer.render_markdown("See [Claw](https://example.com/docs) now.");
        let plain_text = strip_ansi(&markdown_output);

        assert!(plain_text.contains("[Claw](https://example.com/docs)"));
        assert!(markdown_output.contains('\u{1b}'));
    }

    #[test]
    fn highlights_fenced_code_blocks() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output =
            terminal_renderer.markdown_to_ansi("```rust\nfn hi() { println!(\"hi\"); }\n```");
        let plain_text = strip_ansi(&markdown_output);

        assert!(plain_text.contains("╭─ rust"));
        assert!(plain_text.contains("│ "));
        assert!(plain_text.contains("fn hi"));
        assert!(plain_text.contains("╰─"));
        assert!(markdown_output.contains('\u{1b}'));
        assert!(markdown_output.contains("[48;5;236m"));
    }

    #[test]
    fn structures_mixed_prose_and_code_vertically() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output = terminal_renderer.vertical_markdown_to_ansi(
            "I can’t open folders outside the workspace. On macOS, run:\n\n```bash\nopen ~/Downloads\n```\n\nThat opens Finder to your Downloads folder.",
        );
        let plain_text = strip_ansi(&markdown_output);
        let lines: Vec<&str> = plain_text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();

        assert!(
            lines.iter().any(|line| line.starts_with('•') || line.starts_with("- ")),
            "prose should become vertical bullets: {plain_text}"
        );
        assert!(plain_text.contains("╭─ bash"));
        assert!(plain_text.contains("open ~/Downloads"));
        assert!(plain_text.contains("╰─"));
        // Code body should be left-railed, not drifting far right.
        for line in plain_text.lines() {
            if line.contains("open ~/Downloads") {
                let leading = line.len() - line.trim_start().len();
                assert!(
                    leading < 4,
                    "code line should be left-aligned, got leading={leading}: {line:?}"
                );
            }
        }
    }

    #[test]
    fn renders_ordered_and_nested_lists() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output =
            terminal_renderer.render_markdown("1. first\n2. second\n   - nested\n   - child");
        let plain_text = strip_ansi(&markdown_output);

        assert!(plain_text.contains("1. first"));
        assert!(plain_text.contains("2. second"));
        assert!(plain_text.contains("  • nested"));
        assert!(plain_text.contains("  • child"));
    }

    #[test]
    fn vertically_reflows_long_bullets_before_rendering() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output = terminal_renderer.vertical_markdown_to_ansi(
            "- Open and view the file's contents and more detail here\n- Edit or replace text in the file with a longer description",
        );
        let plain_text = strip_ansi(&markdown_output);

        assert!(plain_text
            .lines()
            .any(|line| line.contains("Open and view the file")));
        assert!(plain_text
            .lines()
            .any(|line| line.contains("Edit or replace text")));
        // New bullets start at column 0; wrap continuations may use 2 spaces only.
        for line in plain_text.lines().filter(|l| !l.is_empty()) {
            let lead = line.len() - line.trim_start().len();
            assert!(
                lead == 0 || lead == 2,
                "staircase indent in: {line:?} (lead={lead})"
            );
            if line.trim_start().starts_with('•') {
                assert_eq!(lead, 0, "bullet must be left-aligned: {line:?}");
            }
        }
    }

    #[test]
    fn renders_plain_prose_as_vertical_bullets() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output = terminal_renderer.vertical_markdown_to_ansi(
            "Just to confirm, would you like me to open and display the contents of the file or directory at /Users/example? Please specify if you want to read a file, list a directory, or perform another action.",
        );
        let plain_text = strip_ansi(&markdown_output);

        assert!(plain_text
            .lines()
            .any(|line| line.starts_with("• Just to confirm")));
        assert!(plain_text
            .lines()
            .any(|line| line.contains("Please specify")));
        for line in plain_text.lines().filter(|l| !l.is_empty()) {
            let lead = line.len() - line.trim_start().len();
            assert!(
                lead == 0 || lead == 2,
                "staircase indent in: {line:?} (lead={lead})"
            );
            if line.trim_start().starts_with('•') {
                assert_eq!(lead, 0, "bullet must be left-aligned: {line:?}");
            }
        }
    }

    #[test]
    fn hello_reply_is_left_aligned_vertical_stack() {
        let terminal_renderer = TerminalRenderer::new();
        let plain = strip_ansi(
            &terminal_renderer.vertical_markdown_to_ansi("Hello!\n\nHow can I help?"),
        );
        let lines: Vec<&str> = plain.lines().filter(|l| !l.is_empty()).collect();
        assert!(lines[0].starts_with("• Hello"), "got: {plain:?}");
        assert!(
            lines.iter().any(|l| l.starts_with('•') && l.contains("How can I help")),
            "got: {plain:?}"
        );
        for line in &lines {
            let lead = line.len() - line.trim_start().len();
            assert_eq!(lead, 0, "bullet lines must be flush left: {line:?}");
        }
    }

    #[test]
    fn renders_tables_with_alignment() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output = terminal_renderer
            .render_markdown("| Name | Value |\n| ---- | ----- |\n| alpha | 1 |\n| beta | 22 |");
        let plain_text = strip_ansi(&markdown_output);
        let lines = plain_text.lines().collect::<Vec<_>>();

        assert_eq!(lines[0], "│ Name  │ Value │");
        assert_eq!(lines[1], "│───────┼───────│");
        assert_eq!(lines[2], "│ alpha │ 1     │");
        assert_eq!(lines[3], "│ beta  │ 22    │");
        assert!(markdown_output.contains('\u{1b}'));
    }

    #[test]
    fn streaming_state_waits_for_complete_blocks() {
        let renderer = TerminalRenderer::new();
        let mut state = MarkdownStreamState::default();

        assert_eq!(state.push(&renderer, "# Heading"), None);
        let flushed = state
            .push(&renderer, "\n\nParagraph\n\n")
            .expect("completed block");
        let plain_text = strip_ansi(&flushed);
        assert!(plain_text.contains("Heading"));
        assert!(plain_text.contains("Paragraph"));

        assert_eq!(state.push(&renderer, "```rust\nfn main() {}\n"), None);
        let code = state
            .push(&renderer, "```\n")
            .expect("closed code fence flushes");
        assert!(strip_ansi(&code).contains("fn main()"));
    }

    #[test]
    fn spinner_advances_frames() {
        release_spinner();
        let terminal_renderer = TerminalRenderer::new();
        let mut spinner = Spinner::new();
        // Bypass delay by aging the spinner.
        spinner.started_at = Instant::now() - Duration::from_millis(400);
        let mut out = Vec::new();
        spinner
            .tick("Working", terminal_renderer.color_theme(), &mut out)
            .expect("tick succeeds");
        spinner
            .tick("Working", terminal_renderer.color_theme(), &mut out)
            .expect("tick succeeds");

        let output = String::from_utf8_lossy(&out);
        assert!(
            output.contains("Working"),
            "expected spinner output, got {output:?}"
        );
    }

    #[test]
    fn spinner_stays_silent_before_delay() {
        release_spinner();
        let terminal_renderer = TerminalRenderer::new();
        let mut spinner = Spinner::new();
        let mut out = Vec::new();
        spinner
            .tick("Working", terminal_renderer.color_theme(), &mut out)
            .expect("tick succeeds");
        assert!(
            out.is_empty(),
            "spinner should not paint before SPINNER_DELAY"
        );
        let _ = SPINNER_DELAY; // referenced for docs/stability
    }

    #[test]
    fn turn_footer_includes_time_and_tokens() {
        let footer = format_turn_footer(Duration::from_millis(1500), 1400, 320, false);
        let plain = strip_ansi(&footer);
        assert!(plain.contains("ready"));
        assert!(plain.contains("1.5s"));
        assert!(plain.contains("1.4k"));
        assert!(plain.contains("320"));
    }

    #[test]
    fn tool_timeline_is_compact() {
        let line = format_tool_timeline_line("read", "src/main.rs");
        let plain = strip_ansi(&line);
        assert!(plain.contains("▸"));
        assert!(plain.contains("read"));
        assert!(plain.contains("src/main.rs"));
        assert!(plain.starts_with("  "));
    }

    #[test]
    fn chrome_banner_color_is_grey() {
        assert_eq!(TerminalTheme::Chrome.banner_color(), Color::Grey);
    }

    #[test]
    fn renders_pretty_diff_blocks() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output = terminal_renderer.markdown_to_ansi(
            "```diff\n- old_code();\n+ new_code();\n@@ hunk @@\ncontext_line\n```",
        );
        let plain_text = strip_ansi(&markdown_output);

        assert!(plain_text.contains("Code Modifications"));
        assert!(plain_text.contains("- old_code();"));
        assert!(plain_text.contains("+ new_code();"));
        assert!(plain_text.contains("@@ hunk @@"));
        assert!(plain_text.contains("context_line"));
    }

    #[test]
    fn renders_pretty_checklists_and_phases() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output =
            terminal_renderer.render_markdown("Phase 1: Planning\n- [x] Task one\n- [ ] Task two");
        let plain_text = strip_ansi(&markdown_output);

        assert!(plain_text.contains("Phase 1: Planning"));
        assert!(plain_text.contains("✔ Task one"));
        assert!(plain_text.contains("● Task two"));
    }
}
