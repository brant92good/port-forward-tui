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
    io::{self, BufWriter, IsTerminal, Stderr, Write},
    time::Duration,
};
pub type Screen = Terminal<CrosstermBackend<FrameWriter<Stderr>>>;
const OUTPUT_BUFFER_BYTES: usize = 64 * 1024;

/// Buffered terminal output whose destruction never retries a failed frame.
/// Completed Ratatui draws flush explicitly; teardown discards any remainder.
pub struct FrameWriter<W: Write>(Option<BufWriter<W>>);
impl<W: Write> FrameWriter<W> {
    pub fn new(writer: W) -> Self {
        Self(Some(BufWriter::with_capacity(OUTPUT_BUFFER_BYTES, writer)))
    }
}
impl<W: Write> Write for FrameWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.as_mut().unwrap().write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.as_mut().unwrap().flush()
    }
}
impl<W: Write> Drop for FrameWriter<W> {
    fn drop(&mut self) {
        if let Some(writer) = self.0.take() {
            let _ = writer.into_parts();
        }
    }
}

fn buffered_backend<W: Write>(writer: W) -> CrosstermBackend<FrameWriter<W>> {
    // Crossterm queues individual cells. Stderr is unbuffered, so writing those
    // cells directly makes a large frame visibly arrive line by line in ConPTY.
    // Ratatui flushes each completed draw; capacity bounds unusually large frames.
    CrosstermBackend::new(FrameWriter::new(writer))
}

fn discard_pending<W: Write>(backend: &mut CrosstermBackend<FrameWriter<W>>, replacement: W) {
    // A failed/partial flush must not be retried by BufWriter::drop after the
    // alternate screen has closed. Later Terminal cursor cleanup has no frame
    // bytes left to spill into the caller's shell.
    *backend = CrosstermBackend::new(FrameWriter(Some(BufWriter::with_capacity(0, replacement))));
}
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
            Ok(Terminal::new(buffered_backend(io::stderr()))?)
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
        // Finish output while still on the alternate screen, then discard any
        // remainder on error before emitting the real leave-screen command.
        let _ = self.terminal.show_cursor();
        discard_pending(self.terminal.backend_mut(), io::stderr());
        let _ = terminal::disable_raw_mode();
        let _ = execute!(
            io::stderr(),
            LeaveAlternateScreen,
            DisableFocusChange,
            DisableBracketedPaste
        );
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

#[cfg(test)]
mod output_tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    #[derive(Default)]
    struct State {
        bytes: Vec<u8>,
        writes: usize,
        flushes: usize,
        short_write: bool,
        fail_write_once: bool,
        fail_flush_once: bool,
    }
    #[derive(Clone, Default)]
    struct Recorder(Rc<RefCell<State>>);
    impl Write for Recorder {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let mut state = self.0.borrow_mut();
            if state.fail_write_once {
                state.fail_write_once = false;
                return Err(io::Error::other("owned transient write failure"));
            }
            let count = if state.short_write {
                bytes.len().min(3)
            } else {
                bytes.len()
            };
            state.bytes.extend_from_slice(&bytes[..count]);
            state.writes += 1;
            if state.short_write {
                state.short_write = false;
                state.fail_write_once = true;
            }
            Ok(count)
        }
        fn flush(&mut self) -> io::Result<()> {
            let mut state = self.0.borrow_mut();
            state.flushes += 1;
            if state.fail_flush_once {
                state.fail_flush_once = false;
                Err(io::Error::other("owned transient flush failure"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn frame_writer_batches_cells_and_flushes_each_completed_frame() {
        let record = Recorder::default();
        let mut backend = buffered_backend(record.clone());
        let frame = "\x1b[2J\x1b[1;1HUnicode \u{958b}\u{767c} frame ".repeat(400);
        for byte in frame.as_bytes() {
            backend.write_all(&[*byte]).unwrap();
        }
        assert_eq!(
            record.0.borrow().writes,
            0,
            "Small cells must stay buffered"
        );
        Write::flush(&mut backend).unwrap();
        assert_eq!(record.0.borrow().bytes, frame.as_bytes());
        assert_eq!(record.0.borrow().writes, 1);
        backend.write_all(b"next frame").unwrap();
        Write::flush(&mut backend).unwrap();
        assert!(record.0.borrow().bytes.ends_with(b"next frame"));
        assert_eq!(record.0.borrow().writes, 2);
    }

    #[test]
    fn frame_writer_teardown_never_replays_failed_frame_after_leaving_screen() {
        for fail_during_write in [true, false] {
            let record = Recorder::default();
            let mut backend = buffered_backend(record.clone());
            backend
                .write_all(b"FRAME-CONTENT-THAT-MUST-NOT-SPILL")
                .unwrap();
            if fail_during_write {
                record.0.borrow_mut().short_write = true;
            } else {
                record.0.borrow_mut().fail_flush_once = true;
            }
            assert!(Write::flush(&mut backend).is_err());
            // The real session invokes this before its raw LeaveAlternateScreen.
            // The transient failure is now gone: an accidental retry would work
            // and expose retained frame bytes in the caller's shell.
            discard_pending(&mut backend, record.clone());
            let before_leave = record.0.borrow().bytes.len();
            let mut raw = record.clone();
            raw.write_all(b"\x1b[?1049l").unwrap();
            drop(backend);
            assert_eq!(&record.0.borrow().bytes[before_leave..], b"\x1b[?1049l");
            if fail_during_write {
                assert_eq!(&record.0.borrow().bytes[..before_leave], b"FRA");
            }
        }
    }
}
