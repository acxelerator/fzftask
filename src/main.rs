mod taskfile;

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Text},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};
use std::{env, error::Error, io};

use taskfile::{LoadError, RequiredVar, Taskfile};

/// A single task entry shown in the left pane.
struct Task {
    name: String,
    desc: Option<String>,
    summary: Option<String>,
    cmds: Vec<String>,
    requires: Vec<RequiredVar>,
}

/// Which pane currently has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Filter,
    Tasks,
}

/// What a key press resulted in.
enum Action {
    /// Keep running.
    None,
    /// Quit without selecting anything.
    Quit,
    /// Close the UI and hand this command to the shell.
    Submit(String),
}

/// The active screen.
enum Mode {
    /// Browsing/filtering the task list.
    Browse,
    /// Collecting values for a task's `requires.vars` before submitting.
    Requires(RequiresState),
}

/// State for the "fill in required variables" flow.
struct RequiresState {
    task_name: String,
    vars: Vec<RequiredVar>,
    /// Values already entered for vars `0..current`.
    answers: Vec<String>,
    /// Index of the variable currently being filled.
    current: usize,
    /// Free-form text buffer for the current (non-enum) variable.
    text: String,
    /// Selection state for the current enum variable.
    list: ListState,
}

impl RequiresState {
    fn new(task_name: String, vars: Vec<RequiredVar>) -> Self {
        let mut s = Self {
            task_name,
            vars,
            answers: Vec::new(),
            current: 0,
            text: String::new(),
            list: ListState::default(),
        };
        s.prepare_current();
        s
    }

    fn current_var(&self) -> &RequiredVar {
        &self.vars[self.current]
    }

    fn current_is_enum(&self) -> bool {
        !self.current_var().enum_values.is_empty()
    }

    /// Reset the per-variable input state when moving to a new variable.
    fn prepare_current(&mut self) {
        self.text.clear();
        if self.current < self.vars.len() && self.current_is_enum() {
            self.list.select(Some(0));
        } else {
            self.list.select(None);
        }
    }

    fn list_next(&mut self) {
        let len = self.current_var().enum_values.len();
        if len == 0 {
            return;
        }
        let i = self.list.selected().map_or(0, |i| (i + 1) % len);
        self.list.select(Some(i));
    }

    fn list_previous(&mut self) {
        let len = self.current_var().enum_values.len();
        if len == 0 {
            return;
        }
        let i = self.list.selected().map_or(0, |i| (i + len - 1) % len);
        self.list.select(Some(i));
    }

    /// Record `value` for the current variable and advance. Returns the final
    /// command once every variable has an answer.
    fn accept(&mut self, value: String) -> Option<String> {
        self.answers.push(value);
        self.current += 1;
        if self.current >= self.vars.len() {
            Some(self.build_command())
        } else {
            self.prepare_current();
            None
        }
    }

    fn build_command(&self) -> String {
        // Variables are emitted as a prefix: `NAME=web ENV=prod task deploy`.
        let mut cmd = String::new();
        for (var, value) in self.vars.iter().zip(&self.answers) {
            cmd.push_str(&format!("{}={} ", var.name, shell_quote(value)));
        }
        cmd.push_str(&format!("task {}", self.task_name));
        cmd
    }

    /// A live preview of the command as it is being assembled: already-answered
    /// variables use their values, the current one shows what is being
    /// entered, and not-yet-reached variables show a `…` placeholder.
    fn preview_command(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        for (i, var) in self.vars.iter().enumerate() {
            let value = if i < self.answers.len() {
                shell_quote(&self.answers[i])
            } else if i == self.current {
                self.current_value_preview()
            } else {
                "…".to_string()
            };
            parts.push(format!("{}={}", var.name, value));
        }
        parts.push(format!("task {}", self.task_name));
        parts.join(" ")
    }

    /// The value preview for the variable currently being filled in.
    fn current_value_preview(&self) -> String {
        if self.current_is_enum() {
            self.list
                .selected()
                .and_then(|i| self.current_var().enum_values.get(i))
                .map(|v| shell_quote(v))
                .unwrap_or_else(|| "…".to_string())
        } else {
            let text = self.text.trim();
            if text.is_empty() {
                "…".to_string()
            } else {
                shell_quote(text)
            }
        }
    }
}

/// Fuzzy match: every character of `needle` appears in `haystack` in order
/// (not necessarily contiguously). Both are expected to be lowercased already.
/// e.g. `delo` matches `deploy`. An empty needle matches everything.
fn fuzzy_match(haystack: &str, needle: &str) -> bool {
    let mut chars = haystack.chars();
    needle.chars().all(|n| chars.any(|h| h == n))
}

/// Quote a value for safe inclusion in the shell command line.
fn shell_quote(value: &str) -> String {
    let safe = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ':' | '@' | '%' | '+'));
    if safe {
        value.to_string()
    } else {
        // Wrap in single quotes, escaping any embedded single quotes.
        format!("'{}'", value.replace('\'', r#"'\''"#))
    }
}

/// Application state.
struct App {
    tasks: Vec<Task>,
    /// Indices into `tasks` that match the current `input`, in display order.
    filtered: Vec<usize>,
    /// The current filter query typed in the bottom input box.
    input: String,
    state: ListState,
    /// The pane that receives key input.
    focus: Focus,
    /// The active screen (browse vs. collecting required variables).
    mode: Mode,
    /// Message shown when no Taskfile could be loaded.
    status: Option<String>,
}

impl App {
    fn new() -> Self {
        let (tasks, status) = match Taskfile::load_from_dir(&current_dir()) {
            Ok(tf) => {
                let tasks: Vec<Task> = tf
                    .tasks
                    .into_iter()
                    .map(|(name, def)| Task {
                        name,
                        desc: def.desc,
                        summary: def.summary,
                        cmds: def.cmds,
                        requires: def.requires,
                    })
                    .collect();
                if tasks.is_empty() {
                    (tasks, Some("Taskfile has no tasks".to_string()))
                } else {
                    (tasks, None)
                }
            }
            Err(LoadError::NotFound) => (
                Vec::new(),
                Some("No Taskfile.yml found in the current directory".to_string()),
            ),
            Err(e) => (Vec::new(), Some(e.to_string())),
        };

        let mut app = Self {
            tasks,
            filtered: Vec::new(),
            input: String::new(),
            state: ListState::default(),
            focus: Focus::Filter,
            mode: Mode::Browse,
            status,
        };
        app.apply_filter();
        app
    }

    /// Handle a key press in the current mode and report what should happen.
    fn on_key(&mut self, code: KeyCode) -> Action {
        if matches!(self.mode, Mode::Requires(_)) {
            self.requires_key(code)
        } else {
            self.browse_key(code)
        }
    }

    /// Key handling for the task browser.
    fn browse_key(&mut self, code: KeyCode) -> Action {
        match code {
            KeyCode::Esc => return Action::Quit,
            KeyCode::Tab => self.toggle_focus(),
            // Enter confirms the selection. Tasks with required variables jump
            // into the requires flow instead of submitting immediately.
            KeyCode::Enter => {
                let choice = self
                    .selected()
                    .map(|t| (t.name.clone(), t.requires.clone()));
                if let Some((name, requires)) = choice {
                    if requires.is_empty() {
                        return Action::Submit(format!("task {name}"));
                    }
                    self.mode = Mode::Requires(RequiresState::new(name, requires));
                }
            }
            _ => match self.focus {
                Focus::Filter => match code {
                    KeyCode::Char(c) => self.push_char(c),
                    KeyCode::Backspace => self.pop_char(),
                    KeyCode::Up => self.previous(),
                    KeyCode::Down => self.next(),
                    _ => {}
                },
                Focus::Tasks => match code {
                    // j/k navigate too: the tasks pane has no text input.
                    KeyCode::Up | KeyCode::Char('k') => self.previous(),
                    KeyCode::Down | KeyCode::Char('j') => self.next(),
                    _ => {}
                },
            },
        }
        Action::None
    }

    /// Key handling for the "fill in required variables" flow.
    fn requires_key(&mut self, code: KeyCode) -> Action {
        // Esc cancels the flow and returns to browsing.
        if code == KeyCode::Esc {
            self.mode = Mode::Browse;
            return Action::None;
        }

        let Mode::Requires(state) = &mut self.mode else {
            return Action::None;
        };

        if state.current_is_enum() {
            match code {
                // j/k navigate too, since an enum has no text input to capture.
                KeyCode::Up | KeyCode::Char('k') => state.list_previous(),
                KeyCode::Down | KeyCode::Char('j') => state.list_next(),
                KeyCode::Enter => {
                    let i = state.list.selected().unwrap_or(0);
                    let value = state.current_var().enum_values[i].clone();
                    if let Some(cmd) = state.accept(value) {
                        return Action::Submit(cmd);
                    }
                }
                _ => {}
            }
        } else {
            match code {
                KeyCode::Char(c) => state.text.push(c),
                KeyCode::Backspace => {
                    state.text.pop();
                }
                KeyCode::Enter => {
                    let value = state.text.trim().to_string();
                    // A required variable must not be empty.
                    if !value.is_empty() {
                        if let Some(cmd) = state.accept(value) {
                            return Action::Submit(cmd);
                        }
                    }
                }
                _ => {}
            }
        }
        Action::None
    }

    /// Cycle keyboard focus between the filter box and the task list.
    fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Filter => Focus::Tasks,
            Focus::Tasks => Focus::Filter,
        };
    }

    /// Recompute `filtered` from `input` (case-insensitive fuzzy subsequence
    /// match on task name) and keep the selection within bounds.
    fn apply_filter(&mut self) {
        let query = self.input.to_lowercase();
        self.filtered = self
            .tasks
            .iter()
            .enumerate()
            .filter(|(_, t)| fuzzy_match(&t.name.to_lowercase(), &query))
            .map(|(i, _)| i)
            .collect();

        if self.filtered.is_empty() {
            self.state.select(None);
        } else {
            let sel = self.state.selected().unwrap_or(0).min(self.filtered.len() - 1);
            self.state.select(Some(sel));
        }
    }

    fn push_char(&mut self, c: char) {
        self.input.push(c);
        self.apply_filter();
    }

    fn pop_char(&mut self) {
        self.input.pop();
        self.apply_filter();
    }

    fn next(&mut self) {
        if self.filtered.is_empty() {
            return;
        }
        let i = match self.state.selected() {
            Some(i) => (i + 1) % self.filtered.len(),
            None => 0,
        };
        self.state.select(Some(i));
    }

    fn previous(&mut self) {
        if self.filtered.is_empty() {
            return;
        }
        let i = match self.state.selected() {
            Some(i) => (i + self.filtered.len() - 1) % self.filtered.len(),
            None => 0,
        };
        self.state.select(Some(i));
    }

    fn selected(&self) -> Option<&Task> {
        self.state
            .selected()
            .and_then(|i| self.filtered.get(i))
            .and_then(|&task_idx| self.tasks.get(task_idx))
    }
}

fn current_dir() -> std::path::PathBuf {
    env::current_dir().unwrap_or_else(|_| ".".into())
}

/// Handle `--version` / `--help` before touching the terminal. Returns `true`
/// if the program should exit without starting the UI.
fn handle_cli_args() -> bool {
    for arg in env::args().skip(1) {
        match arg.as_str() {
            "-V" | "--version" => {
                println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
                return true;
            }
            "-h" | "--help" => {
                println!(
                    "{name} {version}\n{desc}\n\n\
                     USAGE:\n    {name}\n\n\
                     Run inside a directory containing a Taskfile.yml. Filter tasks by\n\
                     typing, pick one with Enter, and the `task <name>` command is printed\n\
                     to stdout (use the zsh wrapper in shell/fzftask.zsh to load it onto\n\
                     your prompt).\n\n\
                     OPTIONS:\n    -h, --help       Print this help\n    -V, --version    Print version",
                    name = env!("CARGO_PKG_NAME"),
                    version = env!("CARGO_PKG_VERSION"),
                    desc = env!("CARGO_PKG_DESCRIPTION"),
                );
                return true;
            }
            _ => {}
        }
    }
    false
}

fn main() -> Result<(), Box<dyn Error>> {
    if handle_cli_args() {
        return Ok(());
    }

    // Render the TUI to the terminal directly (/dev/tty) so that stdout is
    // reserved for the selected command. This lets a shell wrapper capture the
    // selection with `$(fzftask)` without the UI escape codes leaking into it.
    let tty = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")?;
    let mut out = tty.try_clone()?;

    // setup terminal
    enable_raw_mode()?;
    execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(tty);
    let mut terminal = Terminal::new(backend)?;

    // create app and run it
    let mut app = App::new();
    let res = run_app(&mut terminal, &mut app);

    // restore terminal
    disable_raw_mode()?;
    execute!(out, LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;

    // Emit the selected command on stdout (consumed by the shell wrapper).
    match res {
        Ok(Some(command)) => println!("{command}"),
        Ok(None) => {}
        Err(err) => eprintln!("{err:?}"),
    }

    Ok(())
}

/// Run the UI loop. Returns the command to emit on Enter, or `None` if the
/// user quit without selecting.
fn run_app<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> io::Result<Option<String>> {
    loop {
        terminal.draw(|f| ui(f, app))?;

        if let Event::Key(key) = event::read()? {
            // Only react to key presses (crossterm also emits release events).
            if key.kind != event::KeyEventKind::Press {
                continue;
            }
            match app.on_key(key.code) {
                Action::None => {}
                Action::Quit => return Ok(None),
                Action::Submit(command) => return Ok(Some(command)),
            }
        }
    }
}

fn ui(f: &mut ratatui::Frame, app: &mut App) {
    if matches!(app.mode, Mode::Requires(_)) {
        requires_ui(f, app);
    } else {
        browse_ui(f, app);
    }
}

/// Render the task browser.
fn browse_ui(f: &mut ratatui::Frame, app: &mut App) {
    // Split vertically: main panes on top, then the filter box and command box.
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(3),
            Constraint::Length(3),
        ])
        .split(f.area());

    // Split the top area into two equal columns.
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[0]);

    // Left pane: filtered, selectable task list.
    let items: Vec<ListItem> = app
        .filtered
        .iter()
        .map(|&i| ListItem::new(app.tasks[i].name.clone()))
        .collect();

    let title = format!("tasks ({}/{})", app.filtered.len(), app.tasks.len());
    let tasks_list = List::new(items)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(focus_border(app, Focus::Tasks)),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    f.render_stateful_widget(tasks_list, chunks[0], &mut app.state);

    // Right pane: details of the selected task (or a status message).
    let detail = Paragraph::new(detail_text(app))
        .block(Block::default().title("details").borders(Borders::ALL))
        .style(Style::default().fg(Color::White));

    f.render_widget(detail, chunks[1]);

    // Filter input box.
    let input = Paragraph::new(app.input.as_str())
        .block(
            Block::default()
                .title("filter (Tab to switch pane, Enter to send, Esc to quit)")
                .borders(Borders::ALL)
                .border_style(focus_border(app, Focus::Filter)),
        )
        .style(Style::default().fg(Color::Yellow));

    f.render_widget(input, rows[1]);

    // Command box: live preview of what Enter will send to the shell. Tasks
    // with required variables show a hint that Enter starts an input flow.
    let command = match app.selected() {
        Some(task) if !task.requires.is_empty() => {
            let names: Vec<&str> = task.requires.iter().map(|v| v.name.as_str()).collect();
            format!("task {}  (Enter to set: {})", task.name, names.join(", "))
        }
        Some(task) => format!("task {}", task.name),
        None => "(no task selected)".to_string(),
    };
    let command = Paragraph::new(command)
        .block(Block::default().title("command").borders(Borders::ALL))
        .style(Style::default().fg(Color::Green));

    f.render_widget(command, rows[2]);

    // Show the cursor only while editing the filter.
    if app.focus == Focus::Filter {
        f.set_cursor_position((
            rows[1].x + 1 + app.input.chars().count() as u16,
            rows[1].y + 1,
        ));
    }
}

/// Render the "fill in required variables" screen.
fn requires_ui(f: &mut ratatui::Frame, app: &mut App) {
    let Mode::Requires(state) = &mut app.mode else {
        return;
    };

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // progress header
            Constraint::Min(0),    // enum list or input
            Constraint::Length(3), // live command preview
            Constraint::Length(3), // hint footer
        ])
        .split(f.area());

    // Header: which task and how far through its variables we are.
    let var = state.current_var();
    let header = Paragraph::new(format!(
        "task {} — variable {}/{}: {}",
        state.task_name,
        state.current + 1,
        state.vars.len(),
        var.name,
    ))
    .block(Block::default().title("requires").borders(Borders::ALL))
    .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
    f.render_widget(header, rows[0]);

    if state.current_is_enum() {
        // Enum variable: pick from the candidate values.
        let items: Vec<ListItem> = var
            .enum_values
            .iter()
            .map(|v| ListItem::new(v.clone()))
            .collect();
        let list = List::new(items)
            .block(
                Block::default()
                    .title(format!("select a value for {}", var.name))
                    .borders(Borders::ALL),
            )
            .highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("> ");
        f.render_stateful_widget(list, rows[1], &mut state.list);
    } else {
        // Free-form variable: type a value.
        let input = Paragraph::new(state.text.as_str())
            .block(
                Block::default()
                    .title(format!("enter a value for {}", var.name))
                    .borders(Borders::ALL),
            )
            .style(Style::default().fg(Color::Yellow));
        f.render_widget(input, rows[1]);
        f.set_cursor_position((
            rows[1].x + 1 + state.text.chars().count() as u16,
            rows[1].y + 1,
        ));
    }

    // Live command preview that updates with every keystroke / selection.
    let preview = Paragraph::new(state.preview_command())
        .block(Block::default().title("command").borders(Borders::ALL))
        .style(Style::default().fg(Color::Green));
    f.render_widget(preview, rows[2]);

    let hint = if state.current_is_enum() {
        "↑/↓ or j/k to choose, Enter to confirm, Esc to cancel"
    } else {
        "type a value, Enter to confirm, Esc to cancel"
    };
    let footer = Paragraph::new(hint)
        .block(Block::default().borders(Borders::ALL))
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(footer, rows[3]);
}

/// Border style that highlights the pane when it has focus.
fn focus_border(app: &App, pane: Focus) -> Style {
    if app.focus == pane {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    }
}

/// Build the rich text shown in the right pane.
fn detail_text(app: &App) -> Text<'static> {
    if let Some(task) = app.selected() {
        let mut lines: Vec<Line> = Vec::new();

        if task.desc.is_some() || task.summary.is_some() {
            lines.push(Line::styled(
                "description:",
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            ));
            if let Some(desc) = &task.desc {
                lines.push(Line::from(format!("  {desc}")));
            }
            if let Some(summary) = &task.summary {
                lines.push(Line::from(format!("  {summary}")));
            }
            lines.push(Line::from(""));
        }

        if !task.requires.is_empty() {
            lines.push(Line::styled(
                "requires:",
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            ));
            for var in &task.requires {
                let line = if var.enum_values.is_empty() {
                    format!("  {} (input)", var.name)
                } else {
                    format!("  {} [{}]", var.name, var.enum_values.join(", "))
                };
                lines.push(Line::from(line));
            }
            lines.push(Line::from(""));
        }

        if !task.cmds.is_empty() {
            lines.push(Line::styled(
                "commands:",
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            ));
            for cmd in &task.cmds {
                lines.push(Line::from(format!("  $ {cmd}")));
            }
        }

        Text::from(lines)
    } else {
        Text::from(
            app.status
                .clone()
                .unwrap_or_else(|| "No task selected".to_string()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn var(name: &str, enums: &[&str]) -> RequiredVar {
        RequiredVar {
            name: name.to_string(),
            enum_values: enums.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn requires_flow_builds_command() {
        let vars = vec![var("NAME", &[]), var("ENV", &["dev", "staging", "prod"])];
        let mut state = RequiresState::new("deploy".into(), vars);

        // First (free-form) variable.
        assert!(!state.current_is_enum());
        assert_eq!(state.accept("web".into()), None);

        // Second (enum) variable; accepting it completes the flow.
        assert!(state.current_is_enum());
        let cmd = state.accept("prod".into()).expect("flow should complete");
        assert_eq!(cmd, "NAME=web ENV=prod task deploy");
    }

    #[test]
    fn preview_reflects_in_progress_input() {
        let vars = vec![var("NAME", &[]), var("ENV", &["dev", "staging", "prod"])];
        let mut state = RequiresState::new("deploy".into(), vars);

        // Nothing entered yet: both are placeholders.
        assert_eq!(state.preview_command(), "NAME=… ENV=… task deploy");

        // Typing the first value updates the preview live.
        state.text.push_str("web");
        assert_eq!(state.preview_command(), "NAME=web ENV=… task deploy");

        // After accepting NAME, the enum's highlighted value is previewed.
        assert_eq!(state.accept("web".into()), None);
        assert_eq!(state.preview_command(), "NAME=web ENV=dev task deploy");

        // Moving the selection reflects in the preview.
        state.list_next();
        assert_eq!(state.preview_command(), "NAME=web ENV=staging task deploy");
    }

    #[test]
    fn shell_quote_wraps_unsafe_values() {
        assert_eq!(shell_quote("prod"), "prod");
        assert_eq!(shell_quote("a/b-c.1"), "a/b-c.1");
        assert_eq!(shell_quote("hello world"), "'hello world'");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("it's"), r#"'it'\''s'"#);
    }

    #[test]
    fn fuzzy_match_is_subsequence() {
        // Non-contiguous subsequence matches (the motivating example).
        assert!(fuzzy_match("deploy", "delo"));
        assert!(fuzzy_match("docs:serve", "dsv"));
        assert!(fuzzy_match("build", "bld"));
        // Empty query matches everything.
        assert!(fuzzy_match("anything", ""));
        // Order matters and missing chars fail.
        assert!(!fuzzy_match("deploy", "yold"));
        assert!(!fuzzy_match("build", "z"));
    }
}
