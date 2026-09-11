//! Saved-forward editor. Connection fields and next-view preferences have
//! separate owners; failures never pretend those two files are one transaction.
use crate::{
    auto_open::{self, Preferences},
    screen::{self, ACCENT, Input, MUTED},
    store::{self, Forward, Store},
};
use anyhow::{Result, ensure};
use crossterm::event::{Event, KeyCode};
use ratatui::{
    layout::{Constraint, Layout},
    style::Style,
    widgets::{Clear, Paragraph, Wrap},
};
use serde_json::Value;
use std::{path::Path, sync::mpsc, thread};

#[derive(Clone)]
pub struct Request {
    pub previous: Forward,
    pub rule: Forward,
    pub original_auto: Option<bool>,
    pub enabled: bool,
}
pub struct Outcome {
    pub success: bool,
    pub notice: String,
    pub saved_forward: Option<Forward>,
}

/// Metadata-only edits deliberately never invoke save_forward. Its callback is
/// the existing controller upsert (or foreground owner), including stale CAS.
pub fn save(
    directory: &Path,
    request: Request,
    save_forward: impl FnOnce(&Forward, &Forward) -> Result<Value>,
) -> Outcome {
    let mut saved_forward = None;
    let result = (|| -> Result<()> {
        request.rule.validate()?;
        ensure!(
            request.rule.id == request.previous.id,
            "The favorite identity changed."
        );
        let current = Store::load(directory)?;
        ensure!(
            current
                .settings
                .forwards
                .iter()
                .any(|rule| rule == &request.previous),
            "This favorite changed in another view. Reopen Edit and try again."
        );
        if let Some(original) = request.original_auto {
            ensure!(
                Preferences::load(directory)?.enabled(&request.previous.id) == original,
                "Automatic opening changed in another view. Reopen Edit and try again."
            );
        } else {
            ensure!(
                request.rule != request.previous,
                "Automatic opening is unavailable. Its saved options were not changed."
            );
        }
        if request.rule != request.previous {
            if let Err(error) = save_forward(&request.rule, &request.previous) {
                // Upsert may have saved before a restart error or a lost reply.
                // Reflect only a fresh observed saved value; do not retry the
                // mutation or proceed with the separately owned option write.
                if Store::load(directory).is_ok_and(|current| {
                    current
                        .settings
                        .forwards
                        .iter()
                        .any(|rule| rule == &request.rule)
                }) {
                    saved_forward = Some(request.rule.clone());
                }
                return Err(error);
            }
            saved_forward = Some(request.rule.clone());
        }
        if let Some(original) = request.original_auto
            && original != request.enabled
        {
            auto_open::set_checked(directory, &request.rule, original, request.enabled)?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => Outcome {
            success: true,
            notice: if request.original_auto.is_some() {
                "Favorite saved. Automatic opening applies to the next new view.".into()
            } else {
                "Forward changes saved. Automatic opening is unavailable; its saved options were not changed.".into()
            },
            saved_forward,
        },
        Err(error) => Outcome {
            success: false,
            notice: if saved_forward.is_some() {
                format!("Forward changes saved; automatic opening was not saved: {error:#}")
            } else {
                format!("{error:#}")
            },
            saved_forward,
        },
    }
}

pub struct Model {
    previous: Forward,
    inputs: [Input; 3],
    original_auto: Option<bool>,
    enabled: bool,
    selected: usize,
    error: String,
    unavailable: String,
}
impl Model {
    pub fn new(directory: &Path, previous: Forward) -> Self {
        let (original_auto, unavailable) = match Preferences::load(directory) {
            Ok(p) => (Some(p.enabled(&previous.id)), String::new()),
            Err(error) => (None, format!("Automatic opening unavailable: {error:#}")),
        };
        Self {
            inputs: [
                Input::new(
                    previous
                        .remote_port
                        .map(|port| port.to_string())
                        .unwrap_or_default(),
                ),
                Input::new(previous.local_port.to_string()),
                Input::new(previous.name.clone()),
            ],
            previous,
            original_auto,
            enabled: original_auto.unwrap_or(false),
            selected: 1,
            error: String::new(),
            unavailable,
        }
    }
    fn request(&self) -> Result<Request> {
        let mut rule = if self.previous.is_socks() {
            let local = if self.inputs[1].value.trim().is_empty() {
                1080
            } else {
                store::port(self.inputs[1].value.trim())?
            };
            Forward::socks(local, &self.inputs[2].value)?
        } else {
            let remote = store::port(self.inputs[0].value.trim())?;
            let local = if self.inputs[1].value.trim().is_empty() {
                remote
            } else {
                store::port(self.inputs[1].value.trim())?
            };
            Forward::new(local, remote, &self.inputs[2].value)?
        };
        rule.id = self.previous.id.clone();
        Ok(Request {
            previous: self.previous.clone(),
            rule,
            original_auto: self.original_auto,
            enabled: self.enabled,
        })
    }
    fn apply(&mut self, outcome: &Outcome) {
        if let Some(saved) = &outcome.saved_forward {
            self.previous = saved.clone();
        }
        self.error = outcome.notice.clone();
    }
    fn move_selection(&mut self, backwards: bool) {
        loop {
            self.selected = (self.selected + if backwards { 3 } else { 1 }) % 4;
            if self.selected != 0 || !self.previous.is_socks() {
                break;
            }
        }
    }
}

pub fn render(frame: &mut ratatui::Frame, model: &Model, machine: &str, saving: bool) {
    let area = screen::centered(frame.area(), 80, 20);
    frame.render_widget(Clear, area);
    frame.render_widget(screen::panel(" Edit favorite "), area);
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .margin(1)
    .split(area);
    frame.render_widget(
        Paragraph::new(format!(
            "Server: {machine} · {}",
            if model.previous.is_socks() {
                "SOCKS5 proxy"
            } else {
                "Fixed forward"
            }
        )),
        rows[0],
    );
    let socks = model.previous.is_socks();
    for (index, label) in [
        if socks {
            "Destination"
        } else {
            "App port on server"
        },
        if socks {
            "Proxy port on this computer (blank uses 1080)"
        } else {
            "Port on this computer (blank uses same)"
        },
        "Name",
    ]
    .iter()
    .enumerate()
    {
        let marker = if model.selected == index { ">" } else { " " };
        frame.render_widget(
            Paragraph::new(format!(
                "{marker} {label}\n  {}",
                if socks && index == 0 {
                    "Chosen by each proxy request"
                } else {
                    &model.inputs[index].value
                }
            ))
            .style(Style::default().fg(if model.selected == index {
                ACCENT
            } else {
                MUTED
            })),
            rows[index + 1],
        );
    }
    let marker = if model.selected == 3 { ">" } else { " " };
    let checked = if model.original_auto.is_none() {
        "unavailable"
    } else if model.enabled {
        "x"
    } else {
        " "
    };
    frame.render_widget(
        Paragraph::new(format!(
            "{marker} [{checked}] Open automatically\n    Connect when a new Ports view opens."
        ))
        .style(Style::default().fg(if model.selected == 3 { ACCENT } else { MUTED }))
        .wrap(Wrap { trim: false }),
        rows[4],
    );
    let message = if saving {
        "Saving… Closing waits for this request to finish."
    } else if !model.error.is_empty() {
        &model.error
    } else if !model.unavailable.is_empty() {
        &model.unavailable
    } else {
        "Changing automatic opening does not start or stop this connection."
    };
    frame.render_widget(Paragraph::new(message).wrap(Wrap { trim: false }), rows[5]);
    frame.render_widget(
        Paragraph::new(
            "Tab/Shift+Tab move · Space toggles the checkbox\nEnter saves · Esc cancels",
        )
        .style(Style::default().fg(MUTED)),
        rows[6],
    );
}

/// A single scoped save worker keeps errors and entered values in this dialog.
/// An already-submitted mutation is observed before closing; it is never retried
/// or represented as cancelled while the controller may still commit it.
pub fn edit(
    terminal: &mut screen::Screen,
    machine: &str,
    mut model: Model,
    mut save_request: impl FnMut(Request) -> Outcome + Send,
) -> Result<Option<String>> {
    thread::scope(|scope| {
        let (requests, incoming) = mpsc::sync_channel::<Request>(1);
        let (results, completed) = mpsc::sync_channel::<Outcome>(1);
        scope.spawn(move || {
            while let Ok(request) = incoming.recv() {
                if results.send(save_request(request)).is_err() {
                    break;
                }
            }
        });
        let mut saving = false;
        let mut closing = false;
        let mut partial_notice = None;
        loop {
            if saving {
                match completed.try_recv() {
                    Ok(outcome) => {
                        saving = false;
                        model.apply(&outcome);
                        if outcome.saved_forward.is_some() {
                            partial_notice = Some(outcome.notice.clone());
                        }
                        if outcome.success || closing {
                            return Ok(Some(outcome.notice));
                        }
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        anyhow::bail!("Edit save worker stopped.")
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                }
            }
            terminal.draw(|frame| render(frame, &model, machine, saving))?;
            match screen::key()? {
                Some(Event::Key(key)) if screen::quit(key) || key.code == KeyCode::Esc => {
                    if saving {
                        closing = true;
                    } else {
                        return Ok(partial_notice);
                    }
                }
                _ if saving => {}
                Some(Event::Key(key)) => match key.code {
                    KeyCode::Tab | KeyCode::Down => model.move_selection(false),
                    KeyCode::BackTab | KeyCode::Up => model.move_selection(true),
                    KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right if model.selected == 3 => {
                        if model.original_auto.is_some() {
                            model.enabled = !model.enabled;
                        }
                    }
                    KeyCode::Enter => match model.request() {
                        Ok(request) => {
                            requests.send(request)?;
                            saving = true;
                        }
                        Err(error) => model.error = format!("{error:#}"),
                    },
                    _ if model.selected < 3 => model.inputs[model.selected].key(key),
                    _ => {}
                },
                Some(Event::Paste(text)) if model.selected < 3 => {
                    model.inputs[model.selected].insert(&text)
                }
                _ => {}
            }
        }
    })
}

#[cfg(test)]
mod socks_tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn proxy_editor_skips_remote_field_and_preserves_kind_identity_and_options() {
        let temp = tempfile::tempdir().unwrap();
        let proxy = Forward::socks(1080, "Proxy").unwrap();
        Store::load(temp.path())
            .unwrap()
            .save(vec![proxy.clone()])
            .unwrap();
        let mut model = Model::new(temp.path(), proxy.clone());
        assert_eq!(model.selected, 1);
        model.move_selection(true);
        assert_eq!(model.selected, 3);
        model.move_selection(false);
        assert_eq!(model.selected, 1);
        model.enabled = true;
        let request = model.request().unwrap();
        assert_eq!(request.rule, proxy);
        let outcome = save(temp.path(), request, |_, _| {
            panic!("Checkbox-only edit must not call controller")
        });
        assert!(outcome.success, "{}", outcome.notice);
        assert!(Preferences::load(temp.path()).unwrap().enabled(&proxy.id));
        assert!(!temp.path().join("endpoint.json").exists());
        model.inputs[1] = Input::new("2080");
        model.inputs[2] = Input::new("Changed proxy");
        let request = model.request().unwrap();
        assert!(request.rule.is_socks());
        assert_eq!(request.rule.remote_port, None);
        assert_eq!(request.rule.local_port, 2080);
        assert_eq!(request.rule.id, proxy.id);
    }

    #[test]
    fn proxy_editor_small_view_has_mode_local_port_and_auto_control() {
        let temp = tempfile::tempdir().unwrap();
        let model = Model::new(temp.path(), Forward::socks(1080, "Proxy").unwrap());
        let mut terminal = Terminal::new(TestBackend::new(64, 18)).unwrap();
        terminal
            .draw(|frame| render(frame, &model, "Fixture", false))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let text = (0..18)
            .map(|y| (0..64).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        for expected in [
            "SOCKS5 proxy",
            "Chosen by each proxy request",
            "1080",
            "Open automatically",
            "Enter saves",
        ] {
            assert!(text.contains(expected), "Missing {expected}: {text}");
        }
        assert!(!text.contains("App port on server"));
    }
}
