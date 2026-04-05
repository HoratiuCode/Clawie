use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::cursor::{MoveToColumn, MoveUp};
use crossterm::event::{read, Event as CrosstermEvent, KeyCode as CrosstermKeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::style::{Color, Print, ResetColor, SetForegroundColor};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType};
use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::{CmdKind, Highlighter};
use rustyline::hint::Hinter;
use rustyline::history::DefaultHistory;
use rustyline::validate::Validator;
use rustyline::{
    Cmd, CompletionType, Config, ConditionalEventHandler, Context, EditMode, Editor, Event,
    EventContext, EventHandler, Helper, KeyCode, KeyEvent, Modifiers,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadOutcome {
    Submit(String),
    Cancel,
    Exit,
}

static SLASH_MENU_REQUESTED: AtomicBool = AtomicBool::new(false);

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
    fn handle(&self, evt: &Event, _n: rustyline::RepeatCount, _positive: bool, ctx: &EventContext) -> Option<Cmd> {
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
        if slash_command_prefix(ctx.line(), ctx.pos()).is_some() {
            SLASH_MENU_REQUESTED.store(true, Ordering::Relaxed);
            Some(Cmd::Interrupt)
        } else {
            None
        }
    }
}

pub struct LineEditor {
    prompt: String,
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
            KeyEvent::from('/'),
            EventHandler::Conditional(Box::new(SlashMenuEventHandler)),
        );
        editor.bind_sequence(
            KeyEvent(KeyCode::Tab, Modifiers::NONE),
            EventHandler::Conditional(Box::new(SlashTabMenuEventHandler)),
        );

        Self {
            prompt: prompt.into(),
            editor,
        }
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

        SLASH_MENU_REQUESTED.store(false, Ordering::Relaxed);
        if let Some(helper) = self.editor.helper_mut() {
            helper.reset_current_line();
        }

        match self.editor.readline(&self.prompt) {
            Ok(line) => Ok(ReadOutcome::Submit(line)),
            Err(ReadlineError::Interrupted) => {
                if SLASH_MENU_REQUESTED.swap(false, Ordering::Relaxed) {
                    return self.open_slash_command_picker();
                }
                let has_input = !self.current_line().is_empty();
                self.finish_interrupted_read()?;
                if has_input {
                    Ok(ReadOutcome::Cancel)
                } else {
                    Ok(ReadOutcome::Exit)
                }
            }
            Err(ReadlineError::Eof) => {
                self.finish_interrupted_read()?;
                Ok(ReadOutcome::Exit)
            }
            Err(error) => Err(io::Error::other(error)),
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

        let mut stdout = io::stdout();
        writeln!(stdout)?;
        enable_raw_mode()?;

        let picker = (|| -> io::Result<ReadOutcome> {
            let mut selected = 0usize;
            let mut offset = 0usize;
            let window_size = 8usize;
            let mut previous_lines = 0usize;
            loop {
                let visible_end = commands.len().min(offset + window_size);
                let visible = &commands[offset..visible_end];

                if previous_lines > 0 {
                    execute!(stdout, MoveUp(previous_lines as u16))?;
                }
                execute!(
                    stdout,
                    MoveToColumn(0),
                    Clear(ClearType::FromCursorDown)
                )?;

                for (index, command) in visible.iter().enumerate() {
                    let actual_index = offset + index;
                    let is_selected = actual_index == selected;
                    execute!(stdout, Print("  ["))?;
                    if is_selected {
                        execute!(stdout, SetForegroundColor(Color::Green), Print("●"), ResetColor)?;
                    } else {
                        execute!(stdout, Print(" "))?;
                    }
                    execute!(stdout, Print("] "), Print(command), Print("\r\n"))?;
                }
                previous_lines = visible.len();
                stdout.flush()?;

                let event = read()?;
                match event {
                    CrosstermEvent::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                        CrosstermKeyCode::Up => {
                            selected = selected.saturating_sub(1);
                            if selected < offset {
                                offset = selected;
                            }
                        }
                        CrosstermKeyCode::Down => {
                            if selected + 1 < commands.len() {
                                selected += 1;
                                if selected >= offset + window_size {
                                    offset = selected + 1 - window_size;
                                }
                            }
                        }
                        CrosstermKeyCode::Enter | CrosstermKeyCode::Char(' ') => {
                            if previous_lines > 0 {
                                execute!(stdout, MoveUp(previous_lines as u16))?;
                            }
                            execute!(stdout, MoveToColumn(0), Clear(ClearType::FromCursorDown))?;
                            return Ok(ReadOutcome::Submit(commands[selected].clone()));
                        }
                        CrosstermKeyCode::Esc => {
                            if previous_lines > 0 {
                                execute!(stdout, MoveUp(previous_lines as u16))?;
                            }
                            execute!(stdout, MoveToColumn(0), Clear(ClearType::FromCursorDown))?;
                            return Ok(ReadOutcome::Cancel);
                        }
                        CrosstermKeyCode::Char('c')
                            if key
                                .modifiers
                                .contains(crossterm::event::KeyModifiers::CONTROL) =>
                        {
                            if previous_lines > 0 {
                                execute!(stdout, MoveUp(previous_lines as u16))?;
                            }
                            execute!(stdout, MoveToColumn(0), Clear(ClearType::FromCursorDown))?;
                            return Ok(ReadOutcome::Cancel);
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
        })();

        disable_raw_mode()?;
        picker
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

#[cfg(test)]
mod tests {
    use super::{slash_command_prefix, LineEditor, SlashCommandHelper};
    use rustyline::completion::Completer;
    use rustyline::highlight::Highlighter;
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
}
