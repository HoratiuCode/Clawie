use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::cursor::{MoveToColumn, MoveUp};
use crossterm::event::{
    read, Event as CrosstermEvent, KeyCode as CrosstermKeyCode, KeyEventKind, KeyModifiers,
};
use crossterm::execute;
use crossterm::style::Print;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, size, Clear, ClearType};
use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::{CmdKind, Highlighter};
use rustyline::hint::Hinter;
use rustyline::history::DefaultHistory;
use rustyline::validate::Validator;
use rustyline::{
    Cmd, CompletionType, ConditionalEventHandler, Config, Context, EditMode, Editor, Event,
    EventContext, EventHandler, Helper, KeyCode, KeyEvent, Modifiers,
};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadOutcome {
    Submit(String),
    FollowUp(String),
    Cancel,
    Exit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SlashMenuItem {
    label: String,
    value: String,
    description: Option<String>,
}

static SLASH_MENU_REQUESTED: AtomicBool = AtomicBool::new(false);
static FOLLOW_UP_REQUESTED: AtomicBool = AtomicBool::new(false);

pub fn prompt_prefix() -> &'static str {
    "📁 clawie › "
}

/// Renders a compact one-line status bar shown just above the input prompt.
/// Displays the active model name and the interrupt shortcut.
#[must_use]
pub fn render_input_status_bar(model: &str) -> String {
    format!(
        "\x1b[2m  model: {}  ·  Ctrl+C / Esc to interrupt\x1b[0m",
        model
    )
}

pub fn render_prompt_banner() -> String {
    let rows = [
        "📁 Clawie v2".to_string(),
        format!("Prompt              {}", prompt_prefix()),
        "Tab                 opens the slash menu".to_string(),
        "Alt+Enter           queues a follow-up".to_string(),
        "Shift+Enter         inserts a newline".to_string(),
        "Ctrl+J              inserts a newline".to_string(),
    ];
    let width = rows
        .iter()
        .map(|row| row.chars().count())
        .max()
        .unwrap_or(0);
    let border = "─".repeat(width + 2);

    let mut lines = Vec::with_capacity(rows.len() + 2);
    lines.push(format!("╭{}╮", border));
    for row in rows {
        if row.starts_with('─') {
            lines.push(format!("├{}┤", "─".repeat(width + 2)));
        } else {
            lines.push(format!("│ {:<width$} │", row, width = width));
        }
    }
    lines.push(format!("╰{}╯", border));
    lines.join("\n")
}

struct SlashCommandHelper {
    completions: Vec<String>,
    current_line: RefCell<String>,
}

impl SlashCommandHelper {
    fn new(completions: Vec<String>) -> Self {
        Self {
            completions: normalize_completions(completions),
            current_line: RefCell::new(String::new()),
        }
    }

    fn reset_current_line(&self) {
        self.current_line.borrow_mut().clear();
    }

    fn current_line(&self) -> String {
        self.current_line.borrow().clone()
    }

    fn set_current_line(&self, line: &str) {
        let mut current = self.current_line.borrow_mut();
        current.clear();
        current.push_str(line);
    }

    fn set_completions(&mut self, completions: Vec<String>) {
        self.completions = normalize_completions(completions);
    }
}

impl Completer for SlashCommandHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        _line: &str,
        _pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Self::Candidate>)> {
        Ok((0, Vec::new()))
    }
}

impl Hinter for SlashCommandHelper {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, _ctx: &Context<'_>) -> Option<Self::Hint> {
        if pos == line.len() && line.trim().eq_ignore_ascii_case("plan") {
            return Some("\nCreate a plan?".to_string());
        }

        None
    }
}

impl Highlighter for SlashCommandHelper {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        self.set_current_line(line);
        Cow::Borrowed(line)
    }

    fn highlight_char(&self, line: &str, _pos: usize, _kind: CmdKind) -> bool {
        self.set_current_line(line);
        false
    }
}

impl Validator for SlashCommandHelper {}
impl Helper for SlashCommandHelper {}

struct SlashMenuEventHandler;

impl ConditionalEventHandler for SlashMenuEventHandler {
    fn handle(
        &self,
        evt: &Event,
        _n: rustyline::RepeatCount,
        _positive: bool,
        ctx: &EventContext,
    ) -> Option<Cmd> {
        debug_assert_eq!(*evt, Event::from(KeyEvent::from('/')));
        if ctx.line().is_empty() && ctx.pos() == 0 {
            SLASH_MENU_REQUESTED.store(true, Ordering::Relaxed);
            Some(Cmd::Interrupt)
        } else {
            None
        }
    }
}

struct SlashTabMenuEventHandler;

impl ConditionalEventHandler for SlashTabMenuEventHandler {
    fn handle(
        &self,
        _evt: &Event,
        _n: rustyline::RepeatCount,
        _positive: bool,
        ctx: &EventContext,
    ) -> Option<Cmd> {
        if (ctx.line().is_empty() && ctx.pos() == 0)
            || slash_command_prefix(ctx.line(), ctx.pos()).is_some()
        {
            SLASH_MENU_REQUESTED.store(true, Ordering::Relaxed);
            Some(Cmd::Interrupt)
        } else {
            None
        }
    }
}

struct FollowUpEventHandler;

impl ConditionalEventHandler for FollowUpEventHandler {
    fn handle(
        &self,
        _evt: &Event,
        _n: rustyline::RepeatCount,
        _positive: bool,
        _ctx: &EventContext,
    ) -> Option<Cmd> {
        FOLLOW_UP_REQUESTED.store(true, Ordering::Relaxed);
        Some(Cmd::AcceptLine)
    }
}

pub struct LineEditor {
    prompt: String,
    model: String,
    editor: Editor<SlashCommandHelper, DefaultHistory>,
}

impl LineEditor {
    #[must_use]
    pub fn new(prompt: impl Into<String>, completions: Vec<String>) -> Self {
        let config = Config::builder()
            .completion_type(CompletionType::List)
            .edit_mode(EditMode::Emacs)
            .build();
        let mut editor = Editor::<SlashCommandHelper, DefaultHistory>::with_config(config)
            .expect("rustyline editor should initialize");
        editor.set_helper(Some(SlashCommandHelper::new(completions)));
        editor.bind_sequence(KeyEvent(KeyCode::Char('J'), Modifiers::CTRL), Cmd::Newline);
        editor.bind_sequence(KeyEvent(KeyCode::Enter, Modifiers::SHIFT), Cmd::Newline);
        editor.bind_sequence(
            KeyEvent(KeyCode::Enter, Modifiers::ALT),
            EventHandler::Conditional(Box::new(FollowUpEventHandler)),
        );
        editor.bind_sequence(
            KeyEvent::from('/'),
            EventHandler::Conditional(Box::new(SlashMenuEventHandler)),
        );
        editor.bind_sequence(
            KeyEvent(KeyCode::Tab, Modifiers::NONE),
            EventHandler::Conditional(Box::new(SlashTabMenuEventHandler)),
        );

        Self {
            prompt: prompt.into(),
            model: String::new(),
            editor,
        }
    }

    /// Set the active model name to display in the per-prompt status bar.
    pub fn set_model(&mut self, model: impl Into<String>) {
        self.model = model.into();
    }

    pub fn push_history(&mut self, entry: impl Into<String>) {
        let entry = entry.into();
        if entry.trim().is_empty() {
            return;
        }

        let _ = self.editor.add_history_entry(entry);
    }

    pub fn set_completions(&mut self, completions: Vec<String>) {
        if let Some(helper) = self.editor.helper_mut() {
            helper.set_completions(completions);
        }
    }
    pub fn read_line(&mut self) -> io::Result<ReadOutcome> {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return self.read_line_fallback();
        }

        let mut initial_text = String::new();
        loop {
            // Build a three-line prompt: branding on first line, status bar on second, input cursor prompt on third.
            let full_prompt = if self.model.is_empty() {
                self.prompt.clone()
            } else {
                format!(
                    "{}\n{}\n› ",
                    self.prompt.trim_end(),
                    render_input_status_bar(&self.model)
                )
            };

            SLASH_MENU_REQUESTED.store(false, Ordering::Relaxed);
            FOLLOW_UP_REQUESTED.store(false, Ordering::Relaxed);
            if let Some(helper) = self.editor.helper_mut() {
                helper.reset_current_line();
            }

            let readline_res = if initial_text.is_empty() {
                self.editor.readline(&full_prompt)
            } else {
                self.editor
                    .readline_with_initial(&full_prompt, (&initial_text, ""))
            };

            match readline_res {
                Ok(line) => {
                    if FOLLOW_UP_REQUESTED.swap(false, Ordering::Relaxed) {
                        return Ok(ReadOutcome::FollowUp(line));
                    }
                    return Ok(ReadOutcome::Submit(line));
                }
                Err(ReadlineError::Interrupted) => {
                    if SLASH_MENU_REQUESTED.swap(false, Ordering::Relaxed) {
                        match self.open_slash_command_picker()? {
                            ReadOutcome::Submit(selection) => {
                                if selection.starts_with("/model")
                                    || selection.starts_with("/theme")
                                {
                                    return Ok(ReadOutcome::Submit(selection));
                                } else {
                                    initial_text = format!("{selection} ");
                                    continue;
                                }
                            }
                            other => return Ok(other),
                        }
                    }
                    let has_input = !self.current_line().is_empty();
                    self.finish_interrupted_read()?;
                    if has_input {
                        return Ok(ReadOutcome::Cancel);
                    } else {
                        return Ok(ReadOutcome::Exit);
                    }
                }
                Err(ReadlineError::Eof) => {
                    self.finish_interrupted_read()?;
                    return Ok(ReadOutcome::Exit);
                }
                Err(error) => return Err(io::Error::other(error)),
            }
        }
    }
    fn current_line(&self) -> String {
        self.editor
            .helper()
            .map_or_else(String::new, SlashCommandHelper::current_line)
    }

    fn finish_interrupted_read(&mut self) -> io::Result<()> {
        if let Some(helper) = self.editor.helper_mut() {
            helper.reset_current_line();
        }
        let mut stdout = io::stdout();
        writeln!(stdout)
    }

    fn read_line_fallback(&self) -> io::Result<ReadOutcome> {
        let mut stdout = io::stdout();
        write!(stdout, "{}", self.prompt)?;
        stdout.flush()?;

        let mut buffer = String::new();
        let bytes_read = io::stdin().read_line(&mut buffer)?;
        if bytes_read == 0 {
            return Ok(ReadOutcome::Exit);
        }

        while matches!(buffer.chars().last(), Some('\n' | '\r')) {
            buffer.pop();
        }
        Ok(ReadOutcome::Submit(buffer))
    }

    fn open_slash_command_picker(&mut self) -> io::Result<ReadOutcome> {
        let prefix = slash_command_prefix(&self.current_line(), self.current_line().len())
            .unwrap_or("/")
            .to_string();
        let commands = self.slash_menu_commands(&prefix);
        if commands.is_empty() {
            return Ok(ReadOutcome::Cancel);
        }

        if prefix.starts_with("/model") {
            return self.open_model_picker(&prefix);
        }
        if prefix.starts_with("/theme") {
            return self.open_theme_picker(&prefix);
        }

        let items = commands
            .into_iter()
            .map(|command| SlashMenuItem {
                label: command.clone(),
                value: command,
                description: None,
            })
            .collect::<Vec<_>>();
        let Some(selection) = self.pick_slash_menu(items)? else {
            return Ok(ReadOutcome::Cancel);
        };

        if selection == "/model" {
            return self.open_model_picker("/model");
        }
        if selection == "/theme" {
            return self.open_theme_picker("/theme");
        }

        Ok(ReadOutcome::Submit(selection))
    }

    fn open_model_picker(&self, prefix: &str) -> io::Result<ReadOutcome> {
        let Some(selection) = self.pick_slash_menu(self.model_menu_items(prefix))? else {
            return Ok(ReadOutcome::Cancel);
        };

        Ok(ReadOutcome::Submit(selection))
    }

    fn open_theme_picker(&self, prefix: &str) -> io::Result<ReadOutcome> {
        let Some(selection) = self.pick_slash_menu(self.theme_menu_items(prefix))? else {
            return Ok(ReadOutcome::Cancel);
        };

        Ok(ReadOutcome::Submit(selection))
    }

    fn pick_slash_menu(&self, items: Vec<SlashMenuItem>) -> io::Result<Option<String>> {
        let mut stdout = io::stdout();
        writeln!(stdout)?;
        enable_raw_mode()?;

        let picker = (|| -> io::Result<ReadOutcome> {
            let mut selected = 0usize;
            let mut offset = 0usize;
            let mut query = String::new();
            let window_size = 8usize;
            let is_model_picker = items
                .first()
                .is_some_and(|item| item.value.starts_with("/model"));
            let is_theme_picker = items
                .first()
                .is_some_and(|item| item.value.starts_with("/theme"));
            let kind = if is_model_picker {
                MenuKind::Model
            } else if is_theme_picker {
                MenuKind::Theme
            } else {
                MenuKind::Slash
            };
            let mut surface = RetainedMenuSurface::default();
            loop {
                let filtered_indices = filter_slash_menu_indices(&items, &query);
                if selected >= filtered_indices.len() {
                    selected = filtered_indices.len().saturating_sub(1);
                }
                if selected < offset {
                    offset = selected;
                }
                if selected >= offset + window_size {
                    offset = selected + 1 - window_size;
                }
                let terminal_width = size().map_or(80, |(columns, _)| columns as usize);
                let menu = MenuFrame {
                    kind,
                    query: &query,
                    list: SelectListView {
                        items: &items,
                        filtered_indices: &filtered_indices,
                        selected,
                        offset,
                        max_visible: window_size,
                    },
                };
                surface.draw(&mut stdout, menu.render(terminal_width))?;

                let event = read()?;
                match event {
                    CrosstermEvent::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                        CrosstermKeyCode::Up => {
                            if filtered_indices.is_empty() {
                                continue;
                            }
                            selected = if selected == 0 {
                                filtered_indices.len().saturating_sub(1)
                            } else {
                                selected.saturating_sub(1)
                            };
                            if selected < offset {
                                offset = selected;
                            }
                        }
                        CrosstermKeyCode::Down => {
                            if filtered_indices.is_empty() {
                                continue;
                            }
                            selected = (selected + 1) % filtered_indices.len();
                            if selected >= offset + window_size {
                                offset = selected + 1 - window_size;
                            }
                        }
                        CrosstermKeyCode::Enter => {
                            if filtered_indices.is_empty() {
                                continue;
                            }
                            surface.clear(&mut stdout)?;
                            return Ok(ReadOutcome::Submit(
                                items[filtered_indices[selected]].value.clone(),
                            ));
                        }
                        CrosstermKeyCode::Esc => {
                            surface.clear(&mut stdout)?;
                            return Ok(ReadOutcome::Cancel);
                        }
                        CrosstermKeyCode::Char('u')
                            if key.modifiers.contains(KeyModifiers::CONTROL) =>
                        {
                            query.clear();
                            selected = 0;
                            offset = 0;
                        }
                        CrosstermKeyCode::Char('c')
                            if key.modifiers.contains(KeyModifiers::CONTROL) =>
                        {
                            surface.clear(&mut stdout)?;
                            return Ok(ReadOutcome::Cancel);
                        }
                        CrosstermKeyCode::Backspace => {
                            query.pop();
                            selected = 0;
                            offset = 0;
                        }
                        CrosstermKeyCode::Char(ch)
                            if key.modifiers.is_empty()
                                || key.modifiers.contains(KeyModifiers::SHIFT) =>
                        {
                            query.push(ch);
                            selected = 0;
                            offset = 0;
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
        })();

        disable_raw_mode()?;
        match picker? {
            ReadOutcome::Submit(value) | ReadOutcome::FollowUp(value) => Ok(Some(value)),
            ReadOutcome::Cancel | ReadOutcome::Exit => Ok(None),
        }
    }

    fn slash_menu_commands(&self, prefix: &str) -> Vec<String> {
        let Some(helper) = self.editor.helper() else {
            return Vec::new();
        };

        let mut unique = BTreeSet::new();
        helper
            .completions
            .iter()
            .filter_map(|candidate| candidate.split_whitespace().next())
            .filter(|candidate| prefix == "/" || candidate.starts_with(prefix))
            .filter(|candidate| unique.insert((*candidate).to_string()))
            .map(ToString::to_string)
            .collect()
    }

    fn model_menu_items(&self, prefix: &str) -> Vec<SlashMenuItem> {
        let Some(helper) = self.editor.helper() else {
            return Vec::new();
        };

        let normalized_prefix = if prefix.trim() == "/model" {
            "/model "
        } else {
            prefix
        };

        let mut seen = BTreeSet::new();
        helper
            .completions
            .iter()
            .filter(|candidate| candidate.starts_with("/model "))
            .filter(|candidate| {
                normalized_prefix == "/model " || candidate.starts_with(normalized_prefix)
            })
            .filter_map(|candidate| {
                let model = candidate.trim_start_matches("/model ").trim();
                if model.is_empty() || !seen.insert(model.to_string()) {
                    return None;
                }

                Some(SlashMenuItem {
                    label: format_model_picker_label(model),
                    value: candidate.clone(),
                    description: model_picker_description(model),
                })
            })
            .collect()
    }

    fn theme_menu_items(&self, prefix: &str) -> Vec<SlashMenuItem> {
        let Some(helper) = self.editor.helper() else {
            return Vec::new();
        };

        let normalized_prefix = if prefix.trim() == "/theme" {
            "/theme "
        } else {
            prefix
        };

        let mut seen = BTreeSet::new();
        helper
            .completions
            .iter()
            .filter(|candidate| candidate.starts_with("/theme "))
            .filter(|candidate| {
                normalized_prefix == "/theme " || candidate.starts_with(normalized_prefix)
            })
            .filter_map(|candidate| {
                let theme = candidate.trim_start_matches("/theme ").trim();
                if theme.is_empty() || !seen.insert(theme.to_string()) {
                    return None;
                }

                Some(SlashMenuItem {
                    label: format_theme_picker_label(theme),
                    value: candidate.clone(),
                    description: theme_picker_description(theme),
                })
            })
            .collect()
    }
}

trait TerminalComponent {
    fn render(&self, width: usize) -> Vec<String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuKind {
    Slash,
    Model,
    Theme,
}

impl MenuKind {
    fn title(self) -> &'static str {
        match self {
            Self::Slash => "Slash Menu",
            Self::Model => "Model Picker",
            Self::Theme => "Theme Picker",
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            Self::Slash => "Type to filter slash commands, then press Enter",
            Self::Model => "Type to filter accessible models, then press Enter",
            Self::Theme => "Type to filter themes, then press Enter",
        }
    }
}

struct SelectListView<'a> {
    items: &'a [SlashMenuItem],
    filtered_indices: &'a [usize],
    selected: usize,
    offset: usize,
    max_visible: usize,
}

impl<'a> SelectListView<'a> {
    fn visible_indices(&self) -> &'a [usize] {
        let end = self
            .filtered_indices
            .len()
            .min(self.offset + self.max_visible);
        &self.filtered_indices[self.offset..end]
    }
}

impl TerminalComponent for SelectListView<'_> {
    fn render(&self, width: usize) -> Vec<String> {
        if self.filtered_indices.is_empty() {
            return vec![dim("  No matches")];
        }

        let visible = self.visible_indices();
        let primary_width = visible
            .iter()
            .map(|index| visible_width(&self.items[*index].label))
            .max()
            .unwrap_or(0)
            .clamp(12, 36);
        let mut lines = Vec::with_capacity(visible.len() + 1);

        for (row, item_index) in visible.iter().enumerate() {
            let actual_index = self.offset + row;
            let selected = actual_index == self.selected;
            let item = &self.items[*item_index];
            let prefix = if selected { "● " } else { "  " };
            let prefix_width = visible_width(prefix);
            let content_width = width.saturating_sub(prefix_width).max(1);
            let label_width = primary_width.min(content_width);
            let label = truncate_to_width(&item.label, label_width);
            let mut line = format!("{prefix}{}", pad_to_width(&label, label_width));

            if let Some(description) = item.description.as_deref() {
                let used = visible_width(&line);
                if content_width > used + 4 {
                    let desc_width = content_width.saturating_sub(used + 2);
                    line.push_str("  ");
                    line.push_str(&dim(&truncate_to_width(description, desc_width)));
                }
            }

            lines.push(if selected { green(&line) } else { line });
        }

        if self.filtered_indices.len() > self.max_visible {
            lines.push(dim(&format!(
                "  {}/{}",
                self.selected.saturating_add(1),
                self.filtered_indices.len()
            )));
        }

        lines
    }
}

struct MenuFrame<'a> {
    kind: MenuKind,
    query: &'a str,
    list: SelectListView<'a>,
}

impl TerminalComponent for MenuFrame<'_> {
    fn render(&self, terminal_width: usize) -> Vec<String> {
        let max_width = terminal_width.saturating_sub(4).clamp(36, 96);
        let query_line = if self.query.is_empty() {
            "Search: ".to_string()
        } else {
            format!("Search: {}", self.query)
        };
        let footer = "↑↓ move · Enter select · Backspace edit · Ctrl+U clear · Esc cancel";
        let mut body = vec![
            self.kind.title().to_string(),
            dim(self.kind.subtitle()),
            dim(&query_line),
        ];
        body.extend(self.list.render(max_width.saturating_sub(4)));
        body.push(dim(footer));

        let inner_width = body
            .iter()
            .map(|line| visible_width(line))
            .max()
            .unwrap_or(0)
            .min(max_width.saturating_sub(4))
            .max(34);

        let mut lines = Vec::with_capacity(body.len() + 2);
        lines.push(red(&format!("╭{}╮", "─".repeat(inner_width + 2))));
        for (index, line) in body.iter().enumerate() {
            let content = truncate_to_width(line, inner_width);
            let padded = pad_to_width(&content, inner_width);
            let rendered = if index == 0 {
                red(&format!("│ {padded} │"))
            } else {
                format!("│ {padded} │")
            };
            lines.push(rendered);
        }
        lines.push(red(&format!("╰{}╯", "─".repeat(inner_width + 2))));
        lines
    }
}

#[derive(Default)]
struct RetainedMenuSurface {
    previous_lines: Vec<String>,
}

impl RetainedMenuSurface {
    fn draw(&mut self, out: &mut impl Write, lines: Vec<String>) -> io::Result<()> {
        if self.previous_lines == lines {
            return out.flush();
        }
        if !self.previous_lines.is_empty() {
            execute!(out, MoveUp(self.previous_lines.len() as u16))?;
        }
        execute!(out, MoveToColumn(0), Clear(ClearType::FromCursorDown))?;
        for line in &lines {
            execute!(out, Print(line), Print("\r\n"))?;
        }
        self.previous_lines = lines;
        out.flush()
    }

    fn clear(&mut self, out: &mut impl Write) -> io::Result<()> {
        if !self.previous_lines.is_empty() {
            execute!(out, MoveUp(self.previous_lines.len() as u16))?;
        }
        execute!(out, MoveToColumn(0), Clear(ClearType::FromCursorDown))?;
        self.previous_lines.clear();
        out.flush()
    }
}

fn visible_width(value: &str) -> usize {
    let stripped = strip_ansi(value);
    UnicodeWidthStr::width(stripped.as_str())
}

fn truncate_to_width(value: &str, max_width: usize) -> String {
    if visible_width(value) <= max_width {
        return value.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return "…".to_string();
    }

    let mut output = String::new();
    let mut used = 0usize;
    let target = max_width.saturating_sub(1);
    for ch in strip_ansi(value).chars() {
        let width = UnicodeWidthStr::width(ch.to_string().as_str());
        if used + width > target {
            break;
        }
        output.push(ch);
        used += width;
    }
    output.push('…');
    output
}

fn pad_to_width(value: &str, width: usize) -> String {
    let current = visible_width(value);
    if current >= width {
        value.to_string()
    } else {
        format!("{value}{}", " ".repeat(width - current))
    }
}

fn strip_ansi(value: &str) -> String {
    let mut stripped = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            stripped.push(ch);
        }
    }
    stripped
}

fn red(value: &str) -> String {
    format!("\x1b[31m{value}\x1b[0m")
}

fn green(value: &str) -> String {
    format!("\x1b[32m{value}\x1b[0m")
}

fn dim(value: &str) -> String {
    format!("\x1b[2m{value}\x1b[0m")
}

fn filter_slash_menu_indices(items: &[SlashMenuItem], query: &str) -> Vec<usize> {
    let terms = query
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>();

    if terms.is_empty() {
        return (0..items.len()).collect();
    }

    items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let haystack = format!(
                "{} {} {}",
                item.label,
                item.value,
                item.description.as_deref().unwrap_or("")
            )
            .to_lowercase();
            terms
                .iter()
                .all(|term| haystack.contains(term))
                .then_some(index)
        })
        .collect()
}

fn format_theme_picker_label(theme: &str) -> String {
    match theme {
        "clawie1" | "emoji" => "clawie1".to_string(),
        "chrome" => "chrome".to_string(),
        "classic" => "classic".to_string(),
        other => other.to_string(),
    }
}

fn theme_picker_description(theme: &str) -> Option<String> {
    match theme {
        "clawie1" | "emoji" => Some("red accent + emoji status markers".to_string()),
        "chrome" => Some("black/white + emoji status markers".to_string()),
        "classic" => Some("red accent without emoji status markers".to_string()),
        _ => None,
    }
}

fn slash_command_prefix(line: &str, pos: usize) -> Option<&str> {
    if pos != line.len() {
        return None;
    }

    let prefix = &line[..pos];
    if !prefix.starts_with('/') {
        return None;
    }

    Some(prefix)
}

fn normalize_completions(completions: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    completions
        .into_iter()
        .filter(|candidate| candidate.starts_with('/'))
        .filter(|candidate| seen.insert(candidate.clone()))
        .collect()
}

fn format_model_picker_label(model: &str) -> String {
    let provider = if model.starts_with("claude") || matches!(model, "opus" | "sonnet" | "haiku") {
        "Anthropic"
    } else if model.starts_with("gpt") || model.starts_with("openai/") {
        "OpenAI"
    } else if model.starts_with("grok") {
        "xAI"
    } else if model.starts_with("gemini") || model.starts_with("google/") {
        "Gemini"
    } else if model.starts_with("qwen") || model.starts_with("dashscope/") {
        "DashScope"
    } else if model.starts_with("moonshot") || model.starts_with("kimi") {
        "Kimi"
    } else if model == "codex" {
        "Codex CLI"
    } else {
        "Custom"
    };

    format!("{provider:<10} {model}")
}

fn model_picker_description(model: &str) -> Option<String> {
    let description = if model == "codex" {
        "uses Codex CLI login"
    } else if model.starts_with("dashscope/") || model.starts_with("qwen") {
        "OpenAI-compatible DashScope route"
    } else if model.starts_with("openai/") {
        "configured OpenAI-compatible route"
    } else if model.contains('/') {
        "configured provider model"
    } else {
        return None;
    };
    Some(description.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        filter_slash_menu_indices, format_model_picker_label, format_theme_picker_label,
        prompt_prefix, render_input_status_bar, render_prompt_banner, slash_command_prefix,
        theme_picker_description, LineEditor, SlashCommandHelper, SlashMenuItem,
    };
    use rustyline::completion::Completer;
    use rustyline::highlight::Highlighter;
    use rustyline::hint::Hinter;
    use rustyline::history::{DefaultHistory, History};
    use rustyline::Context;

    #[test]
    fn extracts_terminal_slash_command_prefixes_with_arguments() {
        assert_eq!(slash_command_prefix("/he", 3), Some("/he"));
        assert_eq!(slash_command_prefix("/help me", 8), Some("/help me"));
        assert_eq!(
            slash_command_prefix("/session switch ses", 19),
            Some("/session switch ses")
        );
        assert_eq!(slash_command_prefix("hello", 5), None);
        assert_eq!(slash_command_prefix("/help", 2), None);
    }

    #[test]
    fn disables_builtin_slash_completion_list() {
        let helper = SlashCommandHelper::new(vec![
            "/help".to_string(),
            "/hello".to_string(),
            "/status".to_string(),
        ]);
        let history = DefaultHistory::new();
        let ctx = Context::new(&history);
        let (start, matches) = helper
            .complete("/he", 3, &ctx)
            .expect("completion should work");

        assert_eq!(start, 0);
        assert!(matches.is_empty());
    }

    #[test]
    fn disables_builtin_slash_argument_completion_list() {
        let helper = SlashCommandHelper::new(vec![
            "/model".to_string(),
            "/model opus".to_string(),
            "/model sonnet".to_string(),
            "/session switch alpha".to_string(),
        ]);
        let history = DefaultHistory::new();
        let ctx = Context::new(&history);
        let (start, matches) = helper
            .complete("/model o", 8, &ctx)
            .expect("completion should work");

        assert_eq!(start, 0);
        assert!(matches.is_empty());
    }

    #[test]
    fn ignores_non_slash_command_completion_requests() {
        let helper = SlashCommandHelper::new(vec!["/help".to_string()]);
        let history = DefaultHistory::new();
        let ctx = Context::new(&history);
        let (_, matches) = helper
            .complete("hello", 5, &ctx)
            .expect("completion should work");

        assert!(matches.is_empty());
    }

    #[test]
    fn hints_create_plan_for_plain_plan_input() {
        let helper = SlashCommandHelper::new(Vec::new());
        let history = DefaultHistory::new();
        let ctx = Context::new(&history);

        assert_eq!(
            helper.hint("plan", 4, &ctx),
            Some("\nCreate a plan?".to_string())
        );
        assert_eq!(helper.hint("/plan", 5, &ctx), None);
        assert_eq!(helper.hint("planning", 8, &ctx), None);
    }

    #[test]
    fn tracks_current_buffer_through_highlighter() {
        let helper = SlashCommandHelper::new(Vec::new());
        let _ = helper.highlight("draft", 5);

        assert_eq!(helper.current_line(), "draft");
    }

    #[test]
    fn push_history_ignores_blank_entries() {
        let mut editor = LineEditor::new("> ", vec!["/help".to_string()]);
        editor.push_history("   ");
        editor.push_history("/help");

        assert_eq!(editor.editor.history().len(), 1);
    }

    #[test]
    fn set_completions_replaces_and_normalizes_candidates() {
        let mut editor = LineEditor::new("> ", vec!["/help".to_string()]);
        editor.set_completions(vec![
            "/model opus".to_string(),
            "/model opus".to_string(),
            "status".to_string(),
        ]);

        let helper = editor.editor.helper().expect("helper should exist");
        assert_eq!(helper.completions, vec!["/model opus".to_string()]);
    }

    #[test]
    fn slash_menu_deduplicates_to_top_level_commands() {
        let editor = LineEditor::new(
            "> ",
            vec![
                "/help".to_string(),
                "/model".to_string(),
                "/model gpt-4.1".to_string(),
                "/status".to_string(),
                "/status verbose".to_string(),
            ],
        );

        assert_eq!(
            editor.slash_menu_commands("/"),
            vec![
                "/help".to_string(),
                "/model".to_string(),
                "/status".to_string()
            ]
        );
    }

    #[test]
    fn slash_menu_filter_matches_values_and_labels_case_insensitively() {
        let items = vec![
            SlashMenuItem {
                label: "/help".to_string(),
                value: "/help".to_string(),
                description: None,
            },
            SlashMenuItem {
                label: "OpenAI     gpt-4.1".to_string(),
                value: "/model gpt-4.1".to_string(),
                description: Some("fast OpenAI model".to_string()),
            },
            SlashMenuItem {
                label: "Anthropic  claude-sonnet-4-6".to_string(),
                value: "/model claude-sonnet-4-6".to_string(),
                description: None,
            },
        ];

        assert_eq!(filter_slash_menu_indices(&items, ""), vec![0, 1, 2]);
        assert_eq!(filter_slash_menu_indices(&items, "GPT"), vec![1]);
        assert_eq!(filter_slash_menu_indices(&items, "model sonnet"), vec![2]);
        assert!(filter_slash_menu_indices(&items, "missing").is_empty());
    }

    #[test]
    fn model_menu_uses_full_model_candidates() {
        let editor = LineEditor::new(
            "> ",
            vec![
                "/model".to_string(),
                "/model claude-sonnet-4-6".to_string(),
                "/model gpt-4.1".to_string(),
                "/model grok-3".to_string(),
            ],
        );

        assert_eq!(
            editor.model_menu_items("/model"),
            vec![
                SlashMenuItem {
                    label: "Anthropic  claude-sonnet-4-6".to_string(),
                    value: "/model claude-sonnet-4-6".to_string(),
                    description: None,
                },
                SlashMenuItem {
                    label: "OpenAI     gpt-4.1".to_string(),
                    value: "/model gpt-4.1".to_string(),
                    description: None,
                },
                SlashMenuItem {
                    label: "xAI        grok-3".to_string(),
                    value: "/model grok-3".to_string(),
                    description: None,
                },
            ]
        );
    }

    #[test]
    fn model_picker_label_infers_provider_family() {
        assert_eq!(
            format_model_picker_label("claude-opus-4-6"),
            "Anthropic  claude-opus-4-6"
        );
        assert_eq!(format_model_picker_label("gpt-4.1"), "OpenAI     gpt-4.1");
        assert_eq!(format_model_picker_label("grok-3"), "xAI        grok-3");
        assert_eq!(
            format_model_picker_label("custom-model"),
            "Custom     custom-model"
        );
    }

    #[test]
    fn theme_picker_labels_describe_designs() {
        assert_eq!(format_theme_picker_label("clawie1"), "clawie1");
        assert_eq!(format_theme_picker_label("chrome"), "chrome");
        assert_eq!(format_theme_picker_label("classic"), "classic");
        assert_eq!(
            theme_picker_description("classic").as_deref(),
            Some("red accent without emoji status markers")
        );
    }

    #[test]
    fn theme_menu_items_offer_direct_theme_selection() {
        let editor = LineEditor::new(
            "> ",
            vec![
                "/theme".to_string(),
                "/theme clawie1".to_string(),
                "/theme chrome".to_string(),
                "/theme classic".to_string(),
            ],
        );

        let items = editor.theme_menu_items("/theme");
        let values = items
            .iter()
            .map(|item| item.value.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            values,
            vec!["/theme clawie1", "/theme chrome", "/theme classic"]
        );
    }

    #[test]
    fn prompt_prefix_keeps_clawie_branding() {
        assert!(prompt_prefix().contains("claw"));
        assert!(prompt_prefix().contains("📁"));
    }

    #[test]
    fn prompt_banner_surfaces_composer_shortcuts() {
        let banner = render_prompt_banner();
        assert!(banner.contains("Clawie v2"));
        assert!(banner.contains("Tab"));
        assert!(banner.contains("Shift+Enter"));
        assert!(!banner.contains("Keyboard Shortcuts HUD"));
        assert!(!banner.contains("Session Cost"));
    }

    #[test]
    fn input_status_bar_includes_model_and_interrupt_hint() {
        let bar = render_input_status_bar("claude-3-5-sonnet");
        assert!(bar.contains("claude-3-5-sonnet"));
        assert!(bar.contains("Ctrl+C"));
        assert!(bar.contains("Esc"));
    }
}
