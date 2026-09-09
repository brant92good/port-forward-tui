use anyhow::Result;
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableFocusChange, EnableBracketedPaste, EnableFocusChange,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    },
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
#[cfg(unix)]
use std::sync::{
    OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::{
    io::{self, IsTerminal, Stderr},
    time::Duration,
};
pub type Screen = Terminal<CrosstermBackend<Stderr>>;
pub const ACCENT: Color = Color::Rgb(96, 218, 224);
pub const MUTED: Color = Color::Rgb(153, 164, 182);
pub const BG: Color = Color::Rgb(17, 22, 32);
#[cfg(unix)]
static TERMINATED: AtomicBool = AtomicBool::new(false);
#[cfg(unix)]
static SIGNAL_HANDLER: OnceLock<Result<(), String>> = OnceLock::new();

#[cfg(unix)]
fn install_signal_handler() -> Result<()> {
    // A picker can return and enter the main screen in the same process. Install
    // once, and keep a received termination request across that transition.
    let installed = SIGNAL_HANDLER.get_or_init(|| {
        ctrlc::set_handler(|| TERMINATED.store(true, Ordering::Relaxed))
            .map_err(|error| error.to_string())
    });
    if let Err(error) = installed {
        anyhow::bail!("Cannot register terminal cleanup handler: {error}");
    }
    Ok(())
}
pub fn panel(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(ACCENT))
        .style(Style::default().bg(BG).fg(Color::White))
}
pub struct Session {
    pub terminal: Screen,
}
impl Session {
    pub fn enter() -> Result<Self> {
        anyhow::ensure!(
            io::stdin().is_terminal() && io::stderr().is_terminal(),
            "This screen needs an interactive terminal. Use --json commands in scripts."
        );
        #[cfg(unix)]
        install_signal_handler()?;
        terminal::enable_raw_mode()?;
        let result = (|| -> Result<Screen> {
            execute!(
                io::stderr(),
                EnterAlternateScreen,
                EnableFocusChange,
                EnableBracketedPaste
            )?;
            Ok(Terminal::new(CrosstermBackend::new(io::stderr()))?)
        })();
        match result {
            Ok(terminal) => Ok(Self { terminal }),
            Err(error) => {
                let _ = terminal::disable_raw_mode();
                let _ = execute!(
                    io::stderr(),
                    LeaveAlternateScreen,
                    DisableFocusChange,
                    DisableBracketedPaste
                );
                Err(error)
            }
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(
            io::stderr(),
            LeaveAlternateScreen,
            DisableFocusChange,
            DisableBracketedPaste
        );
        let _ = self.terminal.show_cursor();
    }
}
pub fn key() -> Result<Option<Event>> {
    #[cfg(unix)]
    if TERMINATED.load(Ordering::Relaxed) {
        // Follow the ordinary quit path so owned foreground SSH process groups
        // are dropped/stopped. Do not exit directly from a signal callback.
        return Ok(Some(Event::Key(KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::CONTROL,
        ))));
    }
    if !event::poll(Duration::from_millis(50))? {
        return Ok(None);
    }
    let event = event::read()?;
    if matches!(
        event,
        Event::Key(KeyEvent {
            kind: KeyEventKind::Release,
            ..
        })
    ) {
        return Ok(None);
    }
    Ok(Some(event))
}
pub fn quit(key: KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c' | 'q'))
}
pub fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}
#[derive(Clone, Default)]
pub struct Input {
    pub value: String,
    cursor: usize,
    select_all: bool,
}
impl Input {
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        Self {
            cursor: value.len(),
            value,
            select_all: true,
        }
    }
    pub fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
        self.select_all = false;
    }
    pub fn insert(&mut self, text: &str) {
        if self.select_all {
            self.clear();
        }
        for c in text.chars().filter(|c| !c.is_control()) {
            if self.value.chars().count() >= 512 {
                break;
            }
            self.value.insert(self.cursor, c);
            self.cursor += c.len_utf8();
        }
    }
    pub fn key(&mut self, key: KeyEvent) {
        let previous = || {
            self.value[..self.cursor]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0)
        };
        match key.code {
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.select_all = true;
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.insert(&c.to_string())
            }
            KeyCode::Backspace => {
                if self.select_all {
                    self.clear();
                } else if self.cursor > 0 {
                    let index = previous();
                    self.value.drain(index..self.cursor);
                    self.cursor = index;
                }
            }
            KeyCode::Delete => {
                if self.select_all {
                    self.clear();
                } else if self.cursor < self.value.len() {
                    let end =
                        self.cursor + self.value[self.cursor..].chars().next().unwrap().len_utf8();
                    self.value.drain(self.cursor..end);
                }
            }
            KeyCode::Left => {
                self.cursor = previous();
                self.select_all = false;
            }
            KeyCode::Right => {
                if self.cursor < self.value.len() {
                    self.cursor += self.value[self.cursor..].chars().next().unwrap().len_utf8();
                }
                self.select_all = false;
            }
            KeyCode::Home => {
                self.cursor = 0;
                self.select_all = false;
            }
            KeyCode::End => {
                self.cursor = self.value.len();
                self.select_all = false;
            }
            _ => {}
        }
    }
}
pub fn render_form(
    frame: &mut ratatui::Frame,
    title: &str,
    fields: &[(&str, String)],
    inputs: &[Input],
    selected: usize,
    hint: &str,
) {
    let area = centered(frame.area(), 78, (fields.len() * 3 + 6) as u16);
    frame.render_widget(Clear, area);
    frame.render_widget(panel(title), area);
    let mut constraints = vec![Constraint::Length(3); fields.len()];
    constraints.push(Constraint::Min(2));
    let rows = Layout::vertical(constraints).margin(1).split(area);
    for (index, ((label, _), input)) in fields.iter().zip(inputs).enumerate() {
        let style = Style::default().fg(if index == selected { ACCENT } else { MUTED });
        frame.render_widget(
            Paragraph::new(input.value.as_str())
                .block(Block::bordered().title(*label))
                .style(style),
            rows[index],
        );
    }
    frame.render_widget(
        Paragraph::new(format!(
            "{hint}\nTab/Shift+Tab move · Enter saves · Esc cancels"
        ))
        .style(Style::default().fg(MUTED))
        .wrap(Wrap { trim: false }),
        rows[fields.len()],
    );
}
pub fn form(
    screen: &mut Screen,
    title: &str,
    fields: &[(&str, String)],
    initial: usize,
    hint: &str,
) -> Result<Option<Vec<String>>> {
    let mut inputs = fields
        .iter()
        .map(|(_, value)| Input::new(value))
        .collect::<Vec<_>>();
    let mut selected = initial.min(inputs.len().saturating_sub(1));
    loop {
        screen.draw(|frame| render_form(frame, title, fields, &inputs, selected, hint))?;
        match key()? {
            Some(Event::Key(key)) if quit(key) || key.code == KeyCode::Esc => return Ok(None),
            Some(Event::Key(key)) => match key.code {
                KeyCode::Enter => return Ok(Some(inputs.into_iter().map(|i| i.value).collect())),
                KeyCode::Tab | KeyCode::Down => selected = (selected + 1) % inputs.len(),
                KeyCode::BackTab | KeyCode::Up => {
                    selected = (selected + inputs.len() - 1) % inputs.len()
                }
                _ => inputs[selected].key(key),
            },
            Some(Event::Paste(text)) => inputs[selected].insert(&text),
            _ => {}
        }
    }
}
pub fn message(screen: &mut Screen, title: &str, text: &str, confirmation: bool) -> Result<bool> {
    loop {
        screen.draw(|frame| {
            let area = centered(frame.area(), 86, 24);
            frame.render_widget(Clear, area);
            frame.render_widget(
                Paragraph::new(format!(
                    "{text}\n\n{}",
                    if confirmation {
                        "Y confirms · Esc cancels"
                    } else {
                        "Enter / Esc returns"
                    }
                ))
                .block(panel(title))
                .wrap(Wrap { trim: false }),
                area,
            );
        })?;
        if let Some(Event::Key(key)) = key()? {
            if quit(key) || key.code == KeyCode::Esc || key.code == KeyCode::Char('n') {
                return Ok(false);
            }
            if confirmation && key.code == KeyCode::Char('y') {
                return Ok(true);
            }
            if !confirmation && matches!(key.code, KeyCode::Enter | KeyCode::Char('q' | '?')) {
                return Ok(true);
            }
        }
    }
}
